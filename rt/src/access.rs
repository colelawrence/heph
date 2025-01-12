//! Module to that defines the [`rt::Access`] trait.
//!
//! The types in the module can be used by the actors to get access to runtime,
//! see [`NewActor::RuntimeAccess`] and [`SyncActor::RuntimeAccess`].
//!
//! [`rt::Access`]: crate::Access
//! [`SyncActor::RuntimeAccess`]: heph::actor::SyncActor::RuntimeAccess

use std::future::Future;
use std::mem::replace;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;
use std::{fmt, io, task};

use heph::actor::{self, NewActor, SyncContext};
use heph::actor_ref::ActorRef;
use heph::supervisor::Supervisor;
use mio::{event, Interest};

use crate::process::ProcessId;
use crate::spawn::{ActorOptions, AddActorError, FutureOptions, PrivateSpawn, Spawn};
use crate::trace::{self, Trace};
use crate::{shared, RuntimeRef};

/// Trait to indicate an API needs access to the Heph runtime.
///
/// This is used by various API to get access to the runtime, but its only
/// usable inside the Heph crate.
///
/// Also see [`NewActor::RuntimeAccess`] and [`SyncActor::RuntimeAccess`].
///
/// [`SyncActor::RuntimeAccess`]: heph::actor::SyncActor::RuntimeAccess
///
/// # Notes
///
/// This trait can't be implemented by types outside of the Heph crate.
pub trait Access: PrivateAccess {}

mod private {
    use std::time::Instant;
    use std::{io, task};

    use mio::{event, Interest};

    use crate::process::ProcessId;
    use crate::{trace, RuntimeRef};

    /// Actual trait behind [`rt::Access`].
    ///
    /// [`rt::Access`]: crate::Access
    pub trait PrivateAccess {
        /// Returns the process id.
        fn pid(&self) -> ProcessId;

        /// Changes the process id to `new_pid`, returning the old process id.
        fn change_pid(&mut self, new_pid: ProcessId) -> ProcessId;

        /// Registers the `source`.
        fn register<S>(&mut self, source: &mut S, interest: Interest) -> io::Result<()>
        where
            S: event::Source + ?Sized;

        /// Reregisters the `source`.
        fn reregister<S>(&mut self, source: &mut S, interest: Interest) -> io::Result<()>
        where
            S: event::Source + ?Sized;

        /// Add a deadline.
        fn add_deadline(&mut self, deadline: Instant);

        /// Remove a previously set deadline.
        fn remove_deadline(&mut self, deadline: Instant);

        /// Changes a deadline's pid from `old_pid` the current pid.
        fn change_deadline(&mut self, old_pid: ProcessId, deadline: Instant);

        /// Create a new [`task::Waker`].
        fn new_task_waker(runtime_ref: &mut RuntimeRef, pid: ProcessId) -> task::Waker;

        /// Returns the CPU the thread is bound to, if any.
        fn cpu(&self) -> Option<usize>;

        /// Start timing an event if tracing is enabled, see [`trace::start`].
        fn start_trace(&self) -> Option<trace::EventTiming>;

        /// Finish tracing an event, see [`trace::finish`].
        fn finish_trace(
            &mut self,
            timing: Option<trace::EventTiming>,
            description: &str,
            attributes: &[(&str, &dyn trace::AttributeValue)],
        );
    }
}

pub(crate) use private::PrivateAccess;

/// Provides access to the thread-local runtime.
///
/// This implements the [`Access`] trait, which is required by various APIs to
/// get access to the runtime. Furthermore this gives access to a
/// [`RuntimeRef`] for users.
///
/// This is usually a part of the [`actor::Context`], see it for more
/// information.
///
/// This is an optimised version of [`ThreadSafe`], but doesn't allow the actor
/// to move between threads.
///
/// [`actor::Context`]: heph::actor::Context
#[derive(Clone)]
pub struct ThreadLocal {
    /// Process id of the actor, used as `Token` in registering things, e.g.
    /// a `TcpStream`, with `mio::Poll`.
    pid: ProcessId,
    rt: RuntimeRef,
}

impl ThreadLocal {
    pub(crate) const fn new(pid: ProcessId, rt: RuntimeRef) -> ThreadLocal {
        ThreadLocal { pid, rt }
    }
}

impl Deref for ThreadLocal {
    type Target = RuntimeRef;

    fn deref(&self) -> &Self::Target {
        &self.rt
    }
}

impl DerefMut for ThreadLocal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rt
    }
}

impl Access for ThreadLocal {}

impl PrivateAccess for ThreadLocal {
    fn pid(&self) -> ProcessId {
        self.pid
    }

    fn change_pid(&mut self, new_pid: ProcessId) -> ProcessId {
        replace(&mut self.pid, new_pid)
    }

    fn register<S>(&mut self, source: &mut S, interest: Interest) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.rt.register(source, self.pid.into(), interest)
    }

    fn reregister<S>(&mut self, source: &mut S, interest: Interest) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.rt.reregister(source, self.pid.into(), interest)
    }

    fn add_deadline(&mut self, deadline: Instant) {
        self.rt.add_deadline(self.pid, deadline)
    }

    fn remove_deadline(&mut self, deadline: Instant) {
        self.rt.remove_deadline(self.pid, deadline);
    }

    fn change_deadline(&mut self, old_pid: ProcessId, deadline: Instant) {
        self.rt.change_deadline(old_pid, self.pid, deadline);
    }

    fn new_task_waker(runtime_ref: &mut RuntimeRef, pid: ProcessId) -> task::Waker {
        runtime_ref.new_local_task_waker(pid)
    }

    fn cpu(&self) -> Option<usize> {
        self.rt.cpu()
    }

    fn start_trace(&self) -> Option<trace::EventTiming> {
        self.rt.start_trace()
    }

    fn finish_trace(
        &mut self,
        timing: Option<trace::EventTiming>,
        description: &str,
        attributes: &[(&str, &dyn trace::AttributeValue)],
    ) {
        self.rt
            .finish_trace(timing, self.pid, description, attributes)
    }
}

impl<S, NA> Spawn<S, NA, ThreadLocal> for ThreadLocal
where
    S: Supervisor<NA> + 'static,
    NA: NewActor<RuntimeAccess = ThreadLocal> + 'static,
    NA::Actor: 'static,
{
}

impl<S, NA> PrivateSpawn<S, NA, ThreadLocal> for ThreadLocal
where
    S: Supervisor<NA> + 'static,
    NA: NewActor<RuntimeAccess = ThreadLocal> + 'static,
    NA::Actor: 'static,
{
    fn try_spawn_setup<ArgFn, E>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, E>>
    where
        ArgFn: FnOnce(&mut actor::Context<NA::Message, ThreadLocal>) -> Result<NA::Argument, E>,
    {
        self.rt
            .try_spawn_setup(supervisor, new_actor, arg_fn, options)
    }
}

impl<S, NA> Spawn<S, NA, ThreadSafe> for ThreadLocal
where
    S: Supervisor<NA> + Send + std::marker::Sync + 'static,
    NA: NewActor<RuntimeAccess = ThreadSafe> + Send + std::marker::Sync + 'static,
    NA::Actor: Send + std::marker::Sync + 'static,
    NA::Message: Send,
{
}

impl<S, NA> PrivateSpawn<S, NA, ThreadSafe> for ThreadLocal
where
    S: Supervisor<NA> + Send + std::marker::Sync + 'static,
    NA: NewActor<RuntimeAccess = ThreadSafe> + Send + std::marker::Sync + 'static,
    NA::Actor: Send + std::marker::Sync + 'static,
    NA::Message: Send,
{
    fn try_spawn_setup<ArgFn, E>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, E>>
    where
        ArgFn: FnOnce(&mut actor::Context<NA::Message, ThreadSafe>) -> Result<NA::Argument, E>,
    {
        self.rt
            .try_spawn_setup(supervisor, new_actor, arg_fn, options)
    }
}

impl fmt::Debug for ThreadLocal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ThreadLocal")
    }
}

/// Provides access to the thread-safe parts of the runtime.
///
/// This implements the [`Access`] trait, which is required by various APIs to
/// get access to the runtime.
///
/// This is usually a part of the [`actor::Context`], see it for more
/// information.
///
/// [`actor::Context`]: heph::actor::Context
#[derive(Clone)]
pub struct ThreadSafe {
    /// Process id of the actor, used as `Token` in registering things, e.g.
    /// a `TcpStream`, with `mio::Poll`.
    pid: ProcessId,
    rt: Arc<shared::RuntimeInternals>,
}

impl ThreadSafe {
    pub(crate) const fn new(pid: ProcessId, rt: Arc<shared::RuntimeInternals>) -> ThreadSafe {
        ThreadSafe { pid, rt }
    }

    /// Spawn a thread-safe [`Future`].
    ///
    /// See [`RuntimeRef::spawn_future`] for more documentation.
    pub fn spawn_future<Fut>(&mut self, future: Fut, options: FutureOptions)
    where
        Fut: Future<Output = ()> + Send + std::marker::Sync + 'static,
    {
        self.rt.spawn_future(future, options)
    }
}

impl Access for ThreadSafe {}

impl PrivateAccess for ThreadSafe {
    fn pid(&self) -> ProcessId {
        self.pid
    }

    fn change_pid(&mut self, new_pid: ProcessId) -> ProcessId {
        replace(&mut self.pid, new_pid)
    }

    fn register<S>(&mut self, source: &mut S, interest: Interest) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.rt.register(source, self.pid.into(), interest)
    }

    fn reregister<S>(&mut self, source: &mut S, interest: Interest) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.rt.reregister(source, self.pid.into(), interest)
    }

    fn add_deadline(&mut self, deadline: Instant) {
        self.rt.add_deadline(self.pid, deadline)
    }

    fn remove_deadline(&mut self, deadline: Instant) {
        self.rt.remove_deadline(self.pid, deadline);
    }

    fn change_deadline(&mut self, old_pid: ProcessId, deadline: Instant) {
        self.rt.change_deadline(old_pid, self.pid, deadline);
    }

    fn new_task_waker(runtime_ref: &mut RuntimeRef, pid: ProcessId) -> task::Waker {
        runtime_ref.new_shared_task_waker(pid)
    }

    fn cpu(&self) -> Option<usize> {
        None
    }

    fn start_trace(&self) -> Option<trace::EventTiming> {
        self.rt.start_trace()
    }

    fn finish_trace(
        &mut self,
        timing: Option<trace::EventTiming>,
        description: &str,
        attributes: &[(&str, &dyn trace::AttributeValue)],
    ) {
        self.rt
            .finish_trace(timing, self.pid, description, attributes)
    }
}

impl<S, NA> Spawn<S, NA, ThreadSafe> for ThreadSafe
where
    S: Supervisor<NA> + Send + std::marker::Sync + 'static,
    NA: NewActor<RuntimeAccess = ThreadSafe> + Send + std::marker::Sync + 'static,
    NA::Actor: Send + std::marker::Sync + 'static,
    NA::Message: Send,
{
}

impl<S, NA> PrivateSpawn<S, NA, ThreadSafe> for ThreadSafe
where
    S: Supervisor<NA> + Send + std::marker::Sync + 'static,
    NA: NewActor<RuntimeAccess = ThreadSafe> + Send + std::marker::Sync + 'static,
    NA::Actor: Send + std::marker::Sync + 'static,
    NA::Message: Send,
{
    fn try_spawn_setup<ArgFn, E>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, E>>
    where
        ArgFn: FnOnce(&mut actor::Context<NA::Message, ThreadSafe>) -> Result<NA::Argument, E>,
    {
        self.rt.spawn_setup(supervisor, new_actor, arg_fn, options)
    }
}

impl fmt::Debug for ThreadSafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ThreadSafe")
    }
}

impl<M, RT> Trace for actor::Context<M, RT>
where
    RT: Access,
{
    fn start_trace(&self) -> Option<trace::EventTiming> {
        self.runtime_ref().start_trace()
    }

    fn finish_trace(
        &mut self,
        timing: Option<trace::EventTiming>,
        description: &str,
        attributes: &[(&str, &dyn trace::AttributeValue)],
    ) {
        self.runtime().finish_trace(timing, description, attributes)
    }
}

/// Provides access to the thread-safe parts of the runtime for synchronous
/// actors.
///
/// This implements the [`Access`] trait, which is required by various APIs to
/// get access to the runtime.
///
/// This is usually a part of the [`actor::Context`], see it for more
/// information.
///
/// [`actor::Context`]: heph::actor::Context
#[derive(Clone)]
pub struct Sync {
    rt: Arc<shared::RuntimeInternals>,
    trace_log: Option<trace::Log>,
}

impl Sync {
    pub(crate) const fn new(
        rt: Arc<shared::RuntimeInternals>,
        trace_log: Option<trace::Log>,
    ) -> Sync {
        Sync { rt, trace_log }
    }

    /// Spawn a thread-safe [`Future`].
    ///
    /// See [`RuntimeRef::spawn_future`] for more documentation.
    pub fn spawn_future<Fut>(&mut self, future: Fut, options: FutureOptions)
    where
        Fut: Future<Output = ()> + Send + std::marker::Sync + 'static,
    {
        self.rt.spawn_future(future, options)
    }
}

impl<S, NA> Spawn<S, NA, ThreadSafe> for Sync
where
    S: Supervisor<NA> + Send + std::marker::Sync + 'static,
    NA: NewActor<RuntimeAccess = ThreadSafe> + Send + std::marker::Sync + 'static,
    NA::Actor: Send + std::marker::Sync + 'static,
    NA::Message: Send,
{
}

impl<S, NA> PrivateSpawn<S, NA, ThreadSafe> for Sync
where
    S: Supervisor<NA> + Send + std::marker::Sync + 'static,
    NA: NewActor<RuntimeAccess = ThreadSafe> + Send + std::marker::Sync + 'static,
    NA::Actor: Send + std::marker::Sync + 'static,
    NA::Message: Send,
{
    fn try_spawn_setup<ArgFn, E>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, E>>
    where
        ArgFn: FnOnce(&mut actor::Context<NA::Message, ThreadSafe>) -> Result<NA::Argument, E>,
    {
        self.rt.spawn_setup(supervisor, new_actor, arg_fn, options)
    }
}

impl fmt::Debug for Sync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sync")
    }
}

impl<M> Trace for SyncContext<M, Sync> {
    fn start_trace(&self) -> Option<trace::EventTiming> {
        trace::start(&self.runtime_ref().trace_log)
    }

    fn finish_trace(
        &mut self,
        timing: Option<trace::EventTiming>,
        description: &str,
        attributes: &[(&str, &dyn trace::AttributeValue)],
    ) {
        trace::finish_rt(
            self.runtime().trace_log.as_mut(),
            timing,
            description,
            attributes,
        )
    }
}
