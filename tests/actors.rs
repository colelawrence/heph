extern crate actor;
extern crate futures_core;

use actor::actor::Actor;
use futures_core::task::Context;
use futures_core::{Async, Future, Poll};

mod util;

use util::quick_poll;

struct TestActor {
    value: usize,
}

struct TestMessage;

struct TestFuture<'a> {
    // This line is what this test is all about, the future should be able to
    // reference the actor.
    actor: &'a mut TestActor,
}

impl<'a> Future for TestFuture<'a> {
    type Item = ();
    type Error = ();
    fn poll(&mut self, _ctx: &mut Context) -> Poll<Self::Item, Self::Error> {
        self.actor.value += 1;
        Ok(Async::Ready(()))
    }
}

impl<'a> Actor<'a> for TestActor {
    type Message = TestMessage;
    type Error = ();
    type Future = TestFuture<'a>;
    fn handle(&'a mut self, _msg: Self::Message) -> Self::Future {
        TestFuture { actor: self }
    }
}

#[test]
fn actor_future_may_reference_actor() {
    let mut actor = TestActor { value: 0 };

    {
        let mut future = actor.handle(TestMessage);
        match quick_poll(&mut future) {
            Ok(Async::Ready(())) =>{},
            _ => panic!("expected the future to be ready, but isn't"),
        }
    }
    assert_eq!(actor.value, 1);
}