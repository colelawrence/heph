//! Module with shared runtime internals.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use std::{io, task};

use log::debug;
use mio::{event, Interest, Registry, Token};

use crate::actor::context::ThreadSafe;
use crate::actor::{self, AddActorError, NewActor};
use crate::actor_ref::ActorRef;
use crate::rt::timers::Timers;
use crate::rt::waker::{self, Waker, WakerId};
use crate::rt::{coordinator, ActorOptions, ProcessId};
use crate::supervisor::Supervisor;

mod scheduler;

pub(crate) use scheduler::Scheduler;

// TODO: make all fields in `SharedRuntimeInternal` private.

/// Shared internals of the runtime.
#[derive(Debug)]
pub(crate) struct SharedRuntimeInternal {
    /// Waker id used to create a `Waker` for thread-safe actors.
    pub(crate) waker_id: WakerId,
    /// Scheduler for thread-safe actors.
    pub(crate) scheduler: Scheduler,
    /// Registry for the `Coordinator`'s `Poll` instance.
    registry: Registry,
    // FIXME: `Timers` is not up to this job.
    pub(crate) timers: Mutex<Timers>,
}

impl SharedRuntimeInternal {
    pub(crate) fn new(
        waker_id: WakerId,
        scheduler: Scheduler,
        registry: Registry,
        timers: Mutex<Timers>,
    ) -> Arc<SharedRuntimeInternal> {
        Arc::new(SharedRuntimeInternal {
            waker_id,
            scheduler,
            registry,
            timers,
        })
    }

    /// Returns a new [`task::Waker`] for the thread-safe actor with `pid`.
    pub(crate) fn new_task_waker(&self, pid: ProcessId) -> task::Waker {
        waker::new(self.waker_id, pid)
    }

    pub(crate) fn new_waker(&self, pid: ProcessId) -> Waker {
        Waker::new(self.waker_id, pid)
    }

    /// Register an `event::Source`, see [`mio::Registry::register`].
    pub(crate) fn register<S>(
        &self,
        source: &mut S,
        token: Token,
        interest: Interest,
    ) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.registry.register(source, token, interest)
    }

    /// Reregister an `event::Source`, see [`mio::Registry::reregister`].
    pub(crate) fn reregister<S>(
        &self,
        source: &mut S,
        token: Token,
        interest: Interest,
    ) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.registry.reregister(source, token, interest)
    }

    pub(crate) fn add_deadline(&self, pid: ProcessId, deadline: Instant) {
        self.timers.lock().unwrap().add_deadline(pid, deadline);
        // Ensure that the coordinator isn't polling and misses the deadline.
        self.wake_coordinator()
    }

    /// Waker used to wake the `Coordinator`, but not schedule any particular
    /// process.
    fn wake_coordinator(&self) {
        Waker::new(self.waker_id, coordinator::WAKER.into()).wake()
    }

    pub(crate) fn spawn_setup<S, NA, ArgFn, ArgFnE>(
        self: &Arc<Self>,
        supervisor: S,
        mut new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, ArgFnE>>
    where
        S: Supervisor<NA> + Send + Sync + 'static,
        NA: NewActor<Context = ThreadSafe> + Sync + Send + 'static,
        ArgFn: FnOnce(&mut actor::Context<NA::Message, ThreadSafe>) -> Result<NA::Argument, ArgFnE>,
        NA::Actor: Send + Sync + 'static,
        NA::Message: Send,
    {
        // Setup adding a new process to the scheduler.
        let actor_entry = self.scheduler.add_actor();
        let pid = actor_entry.pid();
        let name = actor::name::<NA::Actor>();
        debug!("spawning thread-safe actor: pid={}, name={}", pid, name);

        // Create our actor context and our actor with it.
        let (manager, sender, receiver) = inbox::Manager::new_small_channel();
        let actor_ref = ActorRef::local(sender);
        let mut ctx = actor::Context::new_shared(pid, receiver, self.clone());
        let arg = arg_fn(&mut ctx).map_err(AddActorError::ArgFn)?;
        let actor = new_actor.new(ctx, arg).map_err(AddActorError::NewActor)?;

        // Add the actor to the scheduler.
        actor_entry.add(
            options.priority(),
            supervisor,
            new_actor,
            actor,
            manager,
            options.is_ready(),
        );

        Ok(actor_ref)
    }
}
