//! Channel communication primitives.
//!
//! All these channels are **not** thread safe, they don't implement [`Send`] or
//! [`Sync`]. If thread safe channels are required the standard library provides
//! channels in the [`std::sync::mpsc`] modules, or look at
//! [`crossbeam_channels`].
//!
//! [`crossbeam_channels`]: https://crates.io/crates/crossbeam-channel
//! [`std::sync::mpsc`]: https://doc.rust-lang.org/nightly/std/sync/mpsc/index.html

use std::pin::Unpin;

pub mod oneshot;

/// Receiving half of the channel was dropped.
///
/// The send value can be retrieved.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct NoReceiver<T>(pub T);

/// Sending halves of the channel were dropped, without more values being
/// available.
///
/// In case of a oneshot channel it means that no value was ever send. In case
/// of bounded and unbounded channels it means no more values will be send and
/// all send values were received.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct NoValue;

/// Create a new oneshot channel.
///
/// See the [`oneshot`] module for more information.
///
/// [`oneshot`]: oneshot/index.html
pub fn oneshot<T: Unpin>() -> (oneshot::Sender<T>, oneshot::Receiver<T>) {
    oneshot::channel()
}
