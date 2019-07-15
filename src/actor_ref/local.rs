//! Module containing the local actor reference.

use std::fmt;

use crate::actor_ref::{ActorRef, Send, SendError};
#[cfg(test)]
use crate::inbox::Inbox;
use crate::inbox::InboxRef;

/// Local actor reference.
///
/// This is a reference to an actor running on the same thread as this reference
/// is located on. This type does not implement `Send` or `Sync`, if this is
/// needed this reference can be [upgraded] to a [machine local actor reference]
/// which is allowed to be send across thread bounds.
///
/// [`ActorSystem`]: crate::system::ActorSystem
/// [upgraded]: crate::actor_ref::ActorRef::upgrade
/// [machine local actor reference]: crate::actor_ref::Machine
#[derive(Clone)]
pub struct Local<M> {
    /// The inbox of the `Actor`, owned by the `ActorProcess`.
    pub(super) inbox: InboxRef<M>,
}

impl<M> Send for Local<M> {
    type Message = M;

    fn send(&mut self, msg: Self::Message) -> Result<(), SendError<Self::Message>> {
        self.inbox
            .try_deliver(msg)
            .map_err(|msg| SendError { message: msg })
    }
}

impl<M> Eq for Local<M> {}

impl<M> PartialEq for Local<M> {
    fn eq(&self, other: &Local<M>) -> bool {
        self.inbox.same_inbox(&other.inbox)
    }
}

impl<M> fmt::Debug for Local<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LocalActorRef")
    }
}

impl<M> ActorRef<Local<M>> {
    /// Create a new `ActorRef` with a shared mailbox.
    pub(crate) const fn new_local(inbox: InboxRef<M>) -> ActorRef<Local<M>> {
        ActorRef::new(Local { inbox })
    }

    /// Get access to the internal inbox, used in testing.
    #[cfg(test)]
    pub(crate) fn get_inbox(&mut self) -> Option<Inbox<M>> {
        self.inner.inbox.upgrade()
    }
}
