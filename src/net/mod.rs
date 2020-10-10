//! Network related types.
//!
//! The network module support two types of protocols:
//!
//! * [Transmission Control Protocol] (TCP) module provides three main types:
//!   * A [TCP stream] between a local and a remote socket.
//!   * A [TCP listening socket], a socket used to listen for connections.
//!   * A [TCP server], listens for connections and starts a new actor for each.
//! * [User Datagram Protocol] (UDP) only provides a single socket type:
//!   * [`UdpSocket`].
//!
//! [Transmission Control Protocol]: crate::net::tcp
//! [TCP stream]: crate::net::TcpStream
//! [TCP listening socket]: crate::net::TcpListener
//! [TCP server]: crate::net::TcpServer
//! [User Datagram Protocol]: crate::net::udp
//! [`UdpSocket`]: crate::net::UdpSocket

use std::mem::MaybeUninit;
use std::slice;

/// A macro to try an I/O function.
///
/// Note that this is used in the tcp and udp modules and has to be defined
/// before them, otherwise this would have been place below.
macro_rules! try_io {
    ($op: expr) => {
        loop {
            match $op {
                Ok(ok) => break Poll::Ready(Ok(ok)),
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break Poll::Pending,
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => break Poll::Ready(Err(err)),
            }
        }
    };
}

// TODO: better name than rpc.
pub mod rpc;
pub mod tcp;
pub mod udp;

#[doc(no_inline)]
pub use tcp::{TcpListener, TcpServer, TcpStream};
#[doc(no_inline)]
pub use udp::UdpSocket;

#[cfg(test)]
mod tests;

/// Trait to make easier to work with slices of (uninitialised) bytes.
///
/// This is implemented for common types such as `&mut[u8]` and `Vec<u8>`, [see
/// below].
///
/// [see below]: #foreign-impls
pub trait Bytes {
    /// Returns itself as a slice of bytes that may or may not be initialised.
    fn as_bytes(&mut self) -> &mut [MaybeUninit<u8>];

    /// Update the length of the byte slice.
    ///
    /// # Safety
    ///
    /// The caller must ensure that at least `n` of the bytes returned by
    /// [`Bytes::as_bytes`] are initialised.
    unsafe fn update_length(&mut self, n: usize);
}

impl<B> Bytes for &mut B
where
    B: Bytes + ?Sized,
{
    fn as_bytes(&mut self) -> &mut [MaybeUninit<u8>] {
        (&mut **self).as_bytes()
    }

    unsafe fn update_length(&mut self, n: usize) {
        (&mut **self).update_length(n)
    }
}

impl Bytes for [MaybeUninit<u8>] {
    fn as_bytes(&mut self) -> &mut [MaybeUninit<u8>] {
        self
    }

    unsafe fn update_length(&mut self, _: usize) {
        // Can't update the length of a slice.
    }
}

impl Bytes for [u8] {
    fn as_bytes(&mut self) -> &mut [MaybeUninit<u8>] {
        // Safety: `MaybeUninit<u8>` is guaranteed to have the same layout as
        // `u8` so it same to cast the pointer.
        unsafe { &mut *(self as *mut [u8] as *mut [MaybeUninit<u8>]) }
    }

    unsafe fn update_length(&mut self, _: usize) {
        // Can't update the length of a slice.
    }
}

/// The implementation for `Vec<u8>` only uses the uninitialised capacity of the
/// vector. In other words the bytes currently in the vector remain untouched.
///
/// # Examples
///
/// The following example shows that the bytes already in the vector remain
/// untouched.
///
/// ```
/// use heph::net::Bytes;
///
/// let mut buf = Vec::with_capacity(100);
/// buf.extend(b"Hello world!");
///
/// write_bytes(b" Hello mars!", &mut buf);
///
/// assert_eq!(&*buf, b"Hello world! Hello mars!");
///
/// fn write_bytes<B>(src: &[u8], mut buf: B) where B: Bytes {
///     // Writes `src` to `buf`.
/// #   let dst = buf.as_bytes();
/// #   let len = std::cmp::min(src.len(), dst.len());
/// #   // Safety: both the src and dst pointers are good. And we've ensured
/// #   // that the length is correct, not overwriting data we don't own or
/// #   // reading data we don't own.
/// #   unsafe {
/// #       std::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr().cast(), len);
/// #       buf.update_length(len);
/// #   }
/// }
/// ```
impl Bytes for Vec<u8> {
    // NOTE: keep this function in sync with the impl below.
    fn as_bytes(&mut self) -> &mut [MaybeUninit<u8>] {
        // Safety: `Vec` ensures the pointer is correct for us. The pointer is
        // at least valid for start + `Vec::capacity` bytes, a range we stay
        // within.
        unsafe {
            let len = self.len();
            let data_ptr = self.as_mut_ptr().add(len).cast();
            // NOTE: `Vec` ensure capacity >= len, so this is always >= 0.
            let capacity_left = self.capacity() - len;
            slice::from_raw_parts_mut(data_ptr, capacity_left)
        }
    }

    unsafe fn update_length(&mut self, n: usize) {
        self.set_len(self.len() + n);
    }
}
