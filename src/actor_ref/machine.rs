//! Module containing the `MachineLocalActorRef`.

use std::fmt;
use std::task::Waker;

use crossbeam_channel::Sender;

use crate::error::SendError;

/// A reference to an actor that can send messages across thread bounds.
///
/// This works the same as other actor references, see the [actor_ref module]
/// for more documentation.
///
/// [actor_ref module]: index.html
///
/// # Notes
///
/// This reference uses much slower operations then `LocalActorRef`, if at all
/// possible prefer to use `LocalActorRef`.
pub struct MachineLocalActorRef<M> {
    /// Sending side of the channel to messages to.
    sender: Sender<M>,
    /// A way to notify the actor of the new message.
    waker: Waker,
}

impl<M> MachineLocalActorRef<M> {
    /// Create a new `MachineLocalActorRef`.
    ///
    /// The `Waker` must wake the same actor the `Sender` is sending to.
    pub(crate) fn new(sender: Sender<M>, waker: Waker) -> MachineLocalActorRef<M> {
        MachineLocalActorRef {
            sender,
            waker,
        }
    }

    /// Send a message to the actor.
    pub fn send<Msg>(&mut self, msg: Msg) -> Result<(), SendError<Msg>>
        where Msg: Into<M>,
    {
        self.sender.send(msg.into());
        self.waker.wake();
        Ok(())
    }
}

impl<M> Clone for MachineLocalActorRef<M> {
    fn clone(&self) -> MachineLocalActorRef<M> {
        MachineLocalActorRef {
            sender: self.sender.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<M> fmt::Debug for MachineLocalActorRef<M> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("MachineLocalActorRef")
            .finish()
    }
}