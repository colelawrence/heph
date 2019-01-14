#![feature(async_await, await_macro, futures_api, never_type)]

use heph::actor::ActorContext;
use heph::supervisor::NoSupervisor;
use heph::system::{ActorSystem, ActorSystemRef, ActorOptions, RuntimeError};

// The creation and running of the actor system is the same as in example 1.
fn main() -> Result<(), RuntimeError> {
    ActorSystem::new()
        .with_setup(add_greeter_actor)
        .run()
}

/// The is the setup function used in the actor system.
fn add_greeter_actor(mut system_ref: ActorSystemRef) -> Result<(), !> {
    // As shown in example 1 actors don't do anything went they are not awoken.
    // In example 1 we send the actor a message to wake it, in the example will
    // use the `schedule` actor option.
    // The `schedule` actor option will wake (schedule) the actor when it is
    // added to the actor system for the first time. This is useful for actors
    // that don't have any (initial) external wakers, for example our
    // `greeter_actor`.
    let actor = greeter_actor as fn(_) -> _;
    system_ref.try_spawn(NoSupervisor, actor, (), ActorOptions {
        schedule: true,
        .. ActorOptions::default()
    })
        // Since we're using an asynchronous function as actor this is safe, see
        // example 1.
        .unwrap();

    Ok(())
}

/// Our greeter actor.
///
/// Note: this needs the `schedule` options when adding it to the actor system.
async fn greeter_actor(_: ActorContext<!>) -> Result<(), !> {
    println!("Hello World");
    Ok(())
}
