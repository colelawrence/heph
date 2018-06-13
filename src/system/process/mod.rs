//! Module containing process related types and implementation.

use system::scheduler::Priority;

/// Process id, or pid, is an unique id for each process in an `ActorSystem`.
///
/// This is also used as `EventedId` to mio.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ProcessId(u64);

/// The trait that represents a process for the `ActorSystem`.
pub trait Process {
    // TODO: provided a way to create a futures::task::Context, maybe by
    // providing an `ActorSystemRef`?

    /// Run the process.
    ///
    /// If this function returns it is assumed that the process is:
    /// - done completely, i.e. it doesn't have to be run anymore, or
    /// - would block, and it made sure it's scheduled at a later point.
    fn run(&mut self);

    /// Get the process id.
    fn id(&self) -> ProcessId;

    /// Get the priority of the process.
    ///
    /// Used in scheduling the process.
    fn priority(&self) -> Priority;
}

/// Internal process type.
///
/// The calls to the process are dynamically dispatched to erase the actual type
/// of the process, this allows the process itself to have a generic type for
/// the `Actor`. But also because the process itself moves around a lot its
/// actually cheaper to allocate it on the heap and move around a fat pointer to
/// it.
pub type ProcessPtr = Box<dyn Process>;