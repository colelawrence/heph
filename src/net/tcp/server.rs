use std::convert::TryFrom;
use std::mem::{forget, size_of};
use std::net::SocketAddr;
use std::os::unix::io::{FromRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};
use std::{fmt, io};

use log::{debug, error};
use mio::net::TcpListener;
use mio::Interest;

use crate::actor::messages::Terminate;
use crate::actor::{self, Actor, AddActorError, NewActor, Spawnable};
use crate::net::TcpStream;
use crate::rt::{ActorOptions, ProcessId, RuntimeAccess, Signal};
use crate::supervisor::Supervisor;

/// A intermediate structure that implements [`NewActor`], creating
/// [`tcp::Server`].
///
/// See [`tcp::Server::setup`] to create this and [`tcp::Server`] for examples.
///
/// [`tcp::Server`]: Server
/// [`tcp::Server::setup`]: Server::setup
#[derive(Debug)]
pub struct ServerSetup<S, NA> {
    /// All fields are in an `Arc` to allow `ServerSetup` to cheaply be cloned
    /// and still be `Send` and `Sync` for use in the setup function of
    /// `Runtime`.
    inner: Arc<ServerSetupInner<S, NA>>,
}

#[derive(Debug)]
struct ServerSetupInner<S, NA> {
    /// The underlying TCP listener.
    ///
    /// NOTE: This is never registered with any `mio::Poll` instance, it is just
    /// used to return an error quickly if we can't create the socket.
    listener: TcpListener,
    /// Address of the `listener`, used to create new sockets.
    address: SocketAddr,
    /// Supervisor for all actors created by `NewActor`.
    supervisor: S,
    /// NewActor used to create an actor for each connection.
    new_actor: NA,
    /// Options used to spawn the actor.
    options: ActorOptions,
}

impl<S, NA, K> NewActor for ServerSetup<S, NA>
where
    S: Supervisor<NA> + Clone + 'static,
    NA: NewActor<Argument = (TcpStream, SocketAddr), Context = K> + Clone + 'static,
    K: RuntimeAccess + Spawnable<S, NA>,
{
    type Message = ServerMessage;
    type Argument = ();
    type Actor = Server<S, NA, K>;
    type Error = io::Error;
    type Context = K;

    fn new(
        &mut self,
        mut ctx: actor::Context<Self::Message, K>,
        _: Self::Argument,
    ) -> Result<Self::Actor, Self::Error> {
        let this = &*self.inner;
        let mut listener = new_listener(&this.address, 1024)?;
        let token = ctx.pid().into();
        ctx.kind()
            .register(&mut listener, token, Interest::READABLE)?;

        Ok(Server {
            ctx,
            listener,
            supervisor: this.supervisor.clone(),
            new_actor: this.new_actor.clone(),
            options: this.options.clone(),
        })
    }
}

fn new_listener(address: &SocketAddr, backlog: libc::c_int) -> io::Result<TcpListener> {
    // Currently Mio doesn't provide a way to set socket options before binding,
    // so we have to do that ourselves. The following is mostly copied from the
    // Mio source, which I also wrote. Most of this code should live in the
    // `socket2` crate, once that is ready.

    let domain = match address {
        SocketAddr::V4(..) => libc::AF_INET,
        SocketAddr::V6(..) => libc::AF_INET6,
    };

    #[cfg(any(target_os = "freebsd", target_os = "linux"))]
    let socket_type = libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    #[cfg(target_vendor = "apple")]
    let socket_type = libc::SOCK_STREAM;

    // Gives a warning for platforms without SOCK_NONBLOCK.
    let socket = syscall!(socket(domain, socket_type, 0))?;

    struct CloseFd {
        fd: RawFd,
    }
    impl Drop for CloseFd {
        fn drop(&mut self) {
            if let Err(err) = syscall!(close(self.fd)) {
                error!("error closing socket: {}", err);
            }
        }
    }
    let socket = CloseFd { fd: socket };

    // For platforms that don't support flags in socket, we need to set the
    // flags ourselves.
    #[cfg(target_vendor = "apple")]
    syscall!(fcntl(socket.fd, libc::F_SETFL, libc::O_NONBLOCK))
        .and_then(|_| syscall!(fcntl(socket.fd, libc::F_SETFD, libc::FD_CLOEXEC)))
        .map(|_| ())?;

    // Mimick `libstd` and set `SO_NOSIGPIPE` on apple systems.
    #[cfg(target_vendor = "apple")]
    syscall!(setsockopt(
        socket.fd,
        libc::SOL_SOCKET,
        libc::SO_NOSIGPIPE,
        &1 as *const libc::c_int as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())?;

    // Set `SO_REUSEADDR` (mirrors what libstd does).
    syscall!(setsockopt(
        socket.fd,
        libc::SOL_SOCKET,
        libc::SO_REUSEADDR,
        &1 as *const libc::c_int as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t,
    ))
    .map(|_| ())?;

    // Finally, the stuff we actually care about: setting `SO_REUSEPORT(_LB)`.
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let reuseport = libc::SO_REUSEPORT;
    #[cfg(target_os = "freebsd")]
    let reuseport = libc::SO_REUSEPORT_LB; // Improved load balancing.
    syscall!(setsockopt(
        socket.fd,
        libc::SOL_SOCKET,
        reuseport,
        &1 as *const libc::c_int as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t,
    ))
    .map(|_| ())?;

    fn socket_addr(addr: &SocketAddr) -> (*const libc::sockaddr, libc::socklen_t) {
        use std::mem::size_of_val;

        match addr {
            SocketAddr::V4(ref addr) => (
                addr as *const _ as *const libc::sockaddr,
                size_of_val(addr) as libc::socklen_t,
            ),
            SocketAddr::V6(ref addr) => (
                addr as *const _ as *const libc::sockaddr,
                size_of_val(addr) as libc::socklen_t,
            ),
        }
    }

    let (raw_addr, raw_addr_length) = socket_addr(&address);
    syscall!(bind(socket.fd, raw_addr, raw_addr_length)).map(|_| ())?;
    syscall!(listen(socket.fd, backlog)).map(|_| ())?;

    let fd = socket.fd;
    forget(socket); // Don't close the file descriptor.
    Ok(unsafe { TcpListener::from_raw_fd(fd) })
}

impl<S, NA> Clone for ServerSetup<S, NA> {
    fn clone(&self) -> ServerSetup<S, NA> {
        ServerSetup {
            inner: self.inner.clone(),
        }
    }
}

/// An actor that starts a new actor for each accepted TCP connection.
///
/// This actor can start as a thread-local or thread-safe actor. When using the
/// thread-local variant one actor runs per worker thread which spawns
/// thread-local actors to handle the [`TcpStream`]s. See the first example
/// below on how to run this `tcp::Server` as a thread-local actor.
///
/// This actor can also run as thread-safe actor in which case it also spawns
/// thread-safe actors. Note however that using thread-*local* version is
/// recommended. The third example below shows how to run the `tcp::Server` as
/// thread-safe actor.
///
/// # Graceful shutdown
///
/// Graceful shutdown is done by sending it a [`Terminate`] message, see below
/// for an example. The TCP server can also handle (shutdown) process signals,
/// see example 2: my_ip (in the examples directory of the source code) for an
/// example of that.
///
/// # Examples
///
/// The following example is a TCP server that writes "Hello World" to the
/// connection, using the server as a thread-local actor.
///
/// ```
/// #![feature(never_type)]
///
/// use std::io;
/// use std::net::SocketAddr;
///
/// use futures_util::AsyncWriteExt;
///
/// use heph::actor::{self, context, NewActor};
/// # use heph::actor::messages::Terminate;
/// use heph::log::error;
/// use heph::net::tcp::{self, TcpStream};
/// use heph::supervisor::{Supervisor, SupervisorStrategy};
/// use heph::rt::options::Priority;
/// use heph::{rt, ActorOptions, Runtime, RuntimeRef};
///
/// fn main() -> Result<(), rt::Error<io::Error>> {
///     // Create and start the Heph runtime.
///     Runtime::new().map_err(rt::Error::map_type)?.with_setup(setup).start()
/// }
///
/// /// In this setup function we'll spawn the TCP server.
/// fn setup(mut runtime_ref: RuntimeRef) -> io::Result<()> {
///     // The address to listen on.
///     let address = "127.0.0.1:7890".parse().unwrap();
///     // Create our TCP server. We'll use the default actor options.
///     let new_actor = conn_actor as fn(_, _, _) -> _;
///     let server = tcp::Server::setup(address, conn_supervisor, new_actor, ActorOptions::default())?;
///
///     // We advice to give the TCP server a low priority to prioritise
///     // handling of ongoing requests over accepting new requests possibly
///     // overloading the system.
///     let options = ActorOptions::default().with_priority(Priority::LOW);
///     # let mut actor_ref =
///     runtime_ref.try_spawn_local(ServerSupervisor, server, (), options)?;
///     # actor_ref <<= Terminate;
///
///     Ok(())
/// }
///
/// /// Our supervisor for the TCP server.
/// #[derive(Copy, Clone, Debug)]
/// struct ServerSupervisor;
///
/// impl<S, NA> Supervisor<tcp::ServerSetup<S, NA>> for ServerSupervisor
/// where
///     // Trait bounds needed by `tcp::ServerSetup`.
///     S: Supervisor<NA> + Clone + 'static,
///     NA: NewActor<Argument = (TcpStream, SocketAddr), Error = !, Context = context::ThreadLocal> + Clone + 'static,
/// {
///     fn decide(&mut self, err: tcp::ServerError<!>) -> SupervisorStrategy<()> {
///         use tcp::ServerError::*;
///         match err {
///             // When we hit an error accepting a connection we'll drop the old
///             // server and create a new one.
///             Accept(err) => {
///                 error!("error accepting new connection: {}", err);
///                 SupervisorStrategy::Restart(())
///             }
///             // Async function never return an error creating a new actor.
///             NewActor(_) => unreachable!(),
///         }
///     }
///
///     fn decide_on_restart_error(&mut self, err: io::Error) -> SupervisorStrategy<()> {
///         // If we can't create a new server we'll stop.
///         error!("error restarting the TCP server: {}", err);
///         SupervisorStrategy::Stop
///     }
///
///     fn second_restart_error(&mut self, _: io::Error) {
///         // We don't restart a second time, so this will never be called.
///         unreachable!();
///     }
/// }
///
/// /// `conn_actor`'s supervisor.
/// fn conn_supervisor(err: io::Error) -> SupervisorStrategy<(TcpStream, SocketAddr)> {
///     error!("error handling connection: {}", err);
///     SupervisorStrategy::Stop
/// }
///
/// /// The actor responsible for a single TCP stream.
/// async fn conn_actor(_ctx: actor::Context<!>, mut stream: TcpStream, address: SocketAddr) -> io::Result<()> {
/// #   drop(address); // Silence dead code warnings.
///     stream.write_all(b"Hello World").await
/// }
/// ```
///
/// The following example shows how the actor can gracefully be shutdown by
/// sending it a [`Terminate`] message.
///
/// ```
/// #![feature(never_type)]
///
/// use std::io;
/// use std::net::SocketAddr;
///
/// use futures_util::AsyncWriteExt;
///
/// # use heph::actor::context;
/// use heph::actor::messages::Terminate;
/// use heph::{actor, NewActor};
/// use heph::log::error;
/// use heph::net::tcp::{self, TcpStream};
/// use heph::supervisor::{Supervisor, SupervisorStrategy};
/// use heph::rt::options::Priority;
/// use heph::{rt, ActorOptions, Runtime, RuntimeRef};
///
/// fn main() -> Result<(), rt::Error<io::Error>> {
///     Runtime::new().map_err(rt::Error::map_type)?.with_setup(setup).start()
/// }
///
/// fn setup(mut runtime_ref: RuntimeRef) -> io::Result<()> {
///     // This uses the same supervisors as in the previous example, not shown
///     // here.
///
///     // Adding the TCP server is the same as in the example above.
///     let new_actor = conn_actor as fn(_, _, _) -> _;
///     let address = "127.0.0.1:7890".parse().unwrap();
///     let server = tcp::Server::setup(address, conn_supervisor, new_actor, ActorOptions::default())?;
///     let options = ActorOptions::default().with_priority(Priority::LOW);
///     let mut server_ref = runtime_ref.try_spawn_local(ServerSupervisor, server, (), options)?;
///
///     // Because the server is just another actor we can send it messages.
///     // Here we'll send it a terminate message so it will gracefully
///     // shutdown.
///     server_ref <<= Terminate;
///
///     Ok(())
/// }
///
/// # /// # Our supervisor for the TCP server.
/// # #[derive(Copy, Clone, Debug)]
/// # struct ServerSupervisor;
/// #
/// # impl<S, NA> Supervisor<tcp::ServerSetup<S, NA>> for ServerSupervisor
/// # where
/// #     S: Supervisor<NA> + Clone + 'static,
/// #     NA: NewActor<Argument = (TcpStream, SocketAddr), Error = !, Context = context::ThreadLocal> + Clone + 'static,
/// # {
/// #     fn decide(&mut self, err: tcp::ServerError<!>) -> SupervisorStrategy<()> {
/// #         use tcp::ServerError::*;
/// #         match err {
/// #             Accept(err) => {
/// #                 error!("error accepting new connection: {}", err);
/// #                 SupervisorStrategy::Restart(())
/// #             }
/// #             NewActor(_) => unreachable!(),
/// #         }
/// #     }
/// #
/// #     fn decide_on_restart_error(&mut self, err: io::Error) -> SupervisorStrategy<()> {
/// #         error!("error restarting the TCP server: {}", err);
/// #         SupervisorStrategy::Stop
/// #     }
/// #
/// #     fn second_restart_error(&mut self, _: io::Error) {
/// #         // We don't restart a second time, so this will never be called.
/// #         unreachable!();
/// #     }
/// # }
/// #
/// # /// # `conn_actor`'s supervisor.
/// # fn conn_supervisor(err: io::Error) -> SupervisorStrategy<(TcpStream, SocketAddr)> {
/// #     error!("error handling connection: {}", err);
/// #     SupervisorStrategy::Stop
/// # }
/// #
/// /// The actor responsible for a single TCP stream.
/// async fn conn_actor(_ctx: actor::Context<!>, mut stream: TcpStream, address: SocketAddr) -> io::Result<()> {
/// #   drop(address); // Silence dead code warnings.
///     stream.write_all(b"Hello World").await
/// }
/// ```
///
/// This example is similar to the first example, but runs the `tcp::Server`
/// actor as thread-safe actor. *It's recommended to run the server as
/// thread-local actor!* This is just an example show its possible.
///
/// ```
/// #![feature(never_type)]
///
/// use std::io;
/// use std::net::SocketAddr;
///
/// use futures_util::AsyncWriteExt;
///
/// use heph::actor::{self, NewActor};
/// use heph::actor::context::ThreadSafe;
/// # use heph::actor::messages::Terminate;
/// use heph::log::error;
/// use heph::net::tcp::{self, TcpStream};
/// use heph::supervisor::{Supervisor, SupervisorStrategy};
/// use heph::rt::options::Priority;
/// use heph::{rt, ActorOptions, Runtime};
///
/// fn main() -> Result<(), rt::Error<io::Error>> {
///     let mut runtime = Runtime::new().map_err(rt::Error::map_type)?;
///
///     // The address to listen on.
///     let address = "127.0.0.1:7890".parse().unwrap();
///     // Create our TCP server. We'll use the default actor options.
///     let new_actor = conn_actor as fn(_, _, _) -> _;
///     let server = tcp::Server::setup(address, conn_supervisor, new_actor, ActorOptions::default())?;
///
///     let options = ActorOptions::default().with_priority(Priority::LOW);
///     # let mut actor_ref =
///     runtime.try_spawn(ServerSupervisor, server, (), options)?;
///     # actor_ref <<= Terminate;
///
///     runtime.start().map_err(rt::Error::map_type)
/// }
///
/// /// Our supervisor for the TCP server.
/// #[derive(Copy, Clone, Debug)]
/// struct ServerSupervisor;
///
/// impl<S, NA> Supervisor<tcp::ServerSetup<S, NA>> for ServerSupervisor
/// where
///     // Trait bounds needed by `tcp::ServerSetup` using a thread-safe actor.
///     S: Supervisor<NA> + Send + Sync + Clone + 'static,
///     NA: NewActor<Argument = (TcpStream, SocketAddr), Error = !, Context = ThreadSafe> + Send + Sync + Clone + 'static,
///     NA::Actor: Send + Sync + 'static,
///     NA::Message: Send,
/// {
///     fn decide(&mut self, err: tcp::ServerError<!>) -> SupervisorStrategy<()> {
///         use tcp::ServerError::*;
///         match err {
///             // When we hit an error accepting a connection we'll drop the old
///             // server and create a new one.
///             Accept(err) => {
///                 error!("error accepting new connection: {}", err);
///                 SupervisorStrategy::Restart(())
///             }
///             // Async function never return an error creating a new actor.
///             NewActor(_) => unreachable!(),
///         }
///     }
///
///     fn decide_on_restart_error(&mut self, err: io::Error) -> SupervisorStrategy<()> {
///         // If we can't create a new server we'll stop.
///         error!("error restarting the TCP server: {}", err);
///         SupervisorStrategy::Stop
///     }
///
///     fn second_restart_error(&mut self, _: io::Error) {
///         // We don't restart a second time, so this will never be called.
///         unreachable!();
///     }
/// }
///
/// /// `conn_actor`'s supervisor.
/// fn conn_supervisor(err: io::Error) -> SupervisorStrategy<(TcpStream, SocketAddr)> {
///     error!("error handling connection: {}", err);
///     SupervisorStrategy::Stop
/// }
///
/// /// The actor responsible for a single TCP stream.
/// async fn conn_actor(_ctx: actor::Context<!, ThreadSafe>, mut stream: TcpStream, address: SocketAddr) -> io::Result<()> {
/// #   drop(address); // Silence dead code warnings.
///     stream.write_all(b"Hello World").await
/// }
#[derive(Debug)]
pub struct Server<S, NA, K> {
    /// Actor context in which this actor is running.
    ctx: actor::Context<ServerMessage, K>,
    /// The underlying TCP listener, backed by Mio.
    listener: TcpListener,
    /// Supervisor for all actors created by `NewActor`.
    supervisor: S,
    /// `NewActor` used to create an actor for each connection.
    new_actor: NA,
    /// Options used to spawn the actor.
    options: ActorOptions,
}

impl<S, NA, K> Server<S, NA, K>
where
    S: Supervisor<NA> + Clone + 'static,
    NA: NewActor<Argument = (TcpStream, SocketAddr), Context = K> + Clone + 'static,
{
    /// Create a new [`ServerSetup`].
    ///
    /// Arguments:
    /// * `address`: the address to listen on.
    /// * `supervisor`: the [`Supervisor`] used to supervise each started actor,
    /// * `new_actor`: the [`NewActor`] implementation to start each actor,
    ///   and
    /// * `options`: the actor options used to spawn the new actors.
    pub fn setup(
        address: SocketAddr,
        supervisor: S,
        new_actor: NA,
        options: ActorOptions,
    ) -> io::Result<ServerSetup<S, NA>> {
        // We create a listener which don't actually use. However it gives a
        // nicer user-experience to get an error up-front rather than $n errors
        // later, where $n is the number of cpu cores when spawning a new server
        // on each worker thread.
        //
        // Also note that we use a backlog of `0`, this is to avoid adding
        // connections to the queue. But it's only a hint (per the POSIX spec),
        // so most OSes actually completely ignore it.
        //
        // In any case any connections that get added to this sockets queue will
        // be dropped (as they are never accepted).
        new_listener(&address, 0).map(|listener| ServerSetup {
            inner: Arc::new(ServerSetupInner {
                listener,
                address,
                supervisor,
                new_actor,
                options,
            }),
        })
    }
}

impl<S, NA, K> Actor for Server<S, NA, K>
where
    S: Supervisor<NA> + Clone + 'static,
    NA: NewActor<Argument = (TcpStream, SocketAddr), Context = K> + Clone + 'static,
    K: RuntimeAccess + Spawnable<S, NA>,
{
    type Error = ServerError<NA::Error>;

    fn try_poll(
        self: Pin<&mut Self>,
        _ctx: &mut task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        // This is safe because only the `RuntimeRef`, `TcpListener` and
        // the `MailBox` are mutably borrowed and all are `Unpin`.
        let &mut Server {
            ref listener,
            ref mut ctx,
            ref supervisor,
            ref new_actor,
            ref options,
        } = unsafe { self.get_unchecked_mut() };

        // See if we need to shutdown.
        //
        // We don't return immediately here because we're using `SO_REUSEPORT`,
        // which on most OSes causes each listener (file descriptor) to have
        // there own accept queue. This means that connections in *our* would be
        // dropped if we would close the file descriptor immediately. So we
        // first accept all pending connections and start actors for them. Note
        // however that there is still a race condition between our last call to
        // `accept` and the time the file descriptor is actually closed,
        // currently we can't avoid this.
        let should_stop = ctx.try_receive_next().is_some();

        // Next start accepting streams.
        loop {
            let (mut stream, addr) = match listener.accept() {
                Ok(ok) => ok,
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue, // Try again.
                Err(err) => return Poll::Ready(Err(ServerError::Accept(err))),
            };
            debug!("tcp::Server accepted connection: remote_address={}", addr);

            let setup_actor = move |pid: ProcessId, ctx: &mut K| {
                ctx.register(
                    &mut stream,
                    pid.into(),
                    Interest::READABLE | Interest::WRITABLE,
                )?;
                Ok((TcpStream { socket: stream }, addr))
            };
            let res = ctx.kind().spawn(
                supervisor.clone(),
                new_actor.clone(),
                setup_actor,
                options.clone(),
            );
            if let Err(err) = res {
                return Poll::Ready(Err(err.into()));
            }
        }

        if should_stop {
            debug!("TCP server received shutdown message, stopping");
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

/// The message type used by [`tcp::Server`].
///
/// The message implements [`From`]`<`[`Terminate`]`>` and
/// [`TryFrom`]`<`[`Signal`]`>` for the message, allowing for graceful shutdown.
///
/// [`tcp::Server`]: Server
#[derive(Debug)]
pub struct ServerMessage {
    // Allow for future expansion.
    inner: (),
}

impl From<Terminate> for ServerMessage {
    fn from(_: Terminate) -> ServerMessage {
        ServerMessage { inner: () }
    }
}

impl TryFrom<Signal> for ServerMessage {
    type Error = ();

    /// Converts [`Signal::Interrupt`], [`Signal::Terminate`] and
    /// [`Signal::Quit`], fails for all other signals (by returning `Err(())`).
    fn try_from(signal: Signal) -> Result<Self, Self::Error> {
        match signal {
            Signal::Interrupt | Signal::Terminate | Signal::Quit => Ok(ServerMessage { inner: () }),
        }
    }
}

/// Error returned by the [`tcp::Server`] actor.
///
/// [`tcp::Server`]: Server
#[derive(Debug)]
pub enum ServerError<E> {
    /// Error accepting TCP stream.
    Accept(io::Error),
    /// Error creating a new actor to handle the TCP stream.
    NewActor(E),
}

// Not part of the public API.
#[doc(hidden)]
impl<E> From<AddActorError<E, io::Error>> for ServerError<E> {
    fn from(err: AddActorError<E, io::Error>) -> ServerError<E> {
        match err {
            AddActorError::NewActor(err) => ServerError::NewActor(err),
            AddActorError::ArgFn(err) => ServerError::Accept(err),
        }
    }
}

impl<E: fmt::Display> fmt::Display for ServerError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ServerError::*;
        match self {
            Accept(ref err) => write!(f, "error accepting TCP stream: {}", err),
            NewActor(ref err) => write!(f, "error creating new actor: {}", err),
        }
    }
}
