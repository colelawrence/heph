//! Short running synchronous threads that are finished before calling
//! [`Runtime::start`] should be collected, and not run for ever.

use std::thread::sleep;
use std::time::Duration;

use heph::actor::SyncContext;
use heph::supervisor::NoSupervisor;
use heph_rt::spawn::SyncActorOptions;
use heph_rt::Runtime;

#[test]
fn issue_323() {
    let mut runtime = Runtime::new().unwrap();

    let actor = actor as fn(_) -> _;
    let options = SyncActorOptions::default();
    runtime
        .spawn_sync_actor(NoSupervisor, actor, (), options)
        .unwrap();

    // Let the synchronous actor complete first.
    sleep(Duration::from_secs(1));

    // This just needs to return and not hang for ever.
    runtime.start().unwrap();
}

/// Short running synchronous actor.
fn actor<RT>(_: SyncContext<(), RT>) {}
