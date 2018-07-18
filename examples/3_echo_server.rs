#![feature(futures_api, never_type, read_initializer)]

extern crate actor;
extern crate env_logger;
#[macro_use]
extern crate log;

use std::io;
use std::task::Poll;
use std::net::SocketAddr;

use actor::actor::{Actor, NewActor, ActorContext, ActorResult, Status};
use actor::io::{AsyncRead, AsyncWrite};
use actor::net::{TcpListener, TcpStream};
use actor::system::{ActorSystemBuilder, ActorOptions, InitiatorOptions};

/// Our actor that will echo back onto the TCP stream.
#[derive(Debug)]
struct EchoActor {
    /// The TCP connection.
    stream: TcpStream,
    /// Buffer to hold the read contents.
    buffer: Vec<u8>,
}

impl Actor for EchoActor {
    // The type of message we can handle, in our case we don't receive messages.
    type Message = !;
    // The type of errors we can generate. Since we're dealing with I/O, errors
    // are to be expected.
    type Error = io::Error;

    fn handle(&mut self, _: &mut ActorContext, _: Self::Message) -> ActorResult<Self::Error> {
        // This actor doesn't receive messages and thus this is never called.
        unreachable!("EchoActor.poll called");
    }

    // For actors used in an `Initiator` this will likely be the starting point.
    fn poll(&mut self, ctx: &mut ActorContext) -> ActorResult<Self::Error> {
        if self.buffer.is_empty() {
            // Initialise the buffer, if required.
            unsafe {
                let cap = self.buffer.capacity();
                self.buffer.set_len(cap);
                self.stream.initializer().initialize(&mut self.buffer);
            }

            // Try to read from stream.
            let r = self.stream.poll_read(&mut ctx.task_ctx(), &mut self.buffer);
            match r {
                // Read everything from the stream, so we're done.
                Poll::Ready(Ok(0)) => Poll::Ready(Ok(Status::Complete)),
                // Move to writing part.
                Poll::Ready(Ok(n)) => {
                    unsafe { self.buffer.set_len(n) };
                    self.poll(ctx)
                },
                Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                Poll::Pending => Poll::Pending,
            }
        } else {
            // Try to echo back the buffer to the stream.
            match self.stream.poll_write(&mut ctx.task_ctx(), &self.buffer) {
                Poll::Ready(Ok(n)) if n == self.buffer.len() => {
                    // Written the entire buffer, so try reading again.
                    self.buffer.truncate(0);
                    self.poll(ctx)
                },
                Poll::Ready(Ok(n)) => {
                    // Not the entire buffer is written, we need to try again
                    // later.
                    self.buffer.drain(0..n);
                    Poll::Pending
                },
                Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

/// In example 2 we used the `actor_factory` function to implement `NewActor`,
/// here we do it manually.
#[derive(Debug)]
struct NewEchoActor {
    buffer_size: usize,
}

impl NewActor for NewEchoActor {
    type Actor = EchoActor;
    type Item = (TcpStream, SocketAddr);

    fn new(&mut self, (stream, address): Self::Item) -> Self::Actor {
        info!("Accepted connection from: {}", address);
        EchoActor { stream, buffer: Vec::with_capacity(self.buffer_size) }
    }
}

fn main() {
    // Enable logging via the `RUST_LOG` environment variable.
    env_logger::init();

    let actor_factory = NewEchoActor { buffer_size: 128 };

    // The remainder of the example, setting up and running the actor system, is
    // the same as example 2.

    let address = "127.0.0.1:7890".parse().unwrap();
    let listener = TcpListener::bind(address, actor_factory, ActorOptions::default())
        .expect("unable to bind TCP listener");

    let mut actor_system = ActorSystemBuilder::default().build()
        .expect("unable to build the actor system");

    actor_system.add_initiator(listener, InitiatorOptions::default())
        .expect("unable to add listener to actor system");

    actor_system.run().expect("unable to run actor system");
}