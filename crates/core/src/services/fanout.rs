// SPDX-License-Identifier: MIT

//! Deliver every event to several services, in order.

use crate::events::{GameEvent, Service};

/// A [`Service`] that forwards each event to every contained service, in the order given.
/// Put the primary (blocking, retrying) service first and non-blocking secondaries after it;
/// a secondary must never stall the loop.
pub struct Fanout(Vec<Box<dyn Service>>);

impl Fanout {
    pub fn new(services: Vec<Box<dyn Service>>) -> Self {
        Self(services)
    }
}

impl Service for Fanout {
    fn handle(&mut self, event: &GameEvent) {
        for service in &mut self.0 {
            service.handle(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Recorder(Arc<Mutex<Vec<String>>>, &'static str);

    impl Service for Recorder {
        fn handle(&mut self, event: &GameEvent) {
            self.0
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.1, event.name()));
        }
    }

    #[test]
    fn delivers_to_every_service_in_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut fanout = Fanout::new(vec![
            Box::new(Recorder(log.clone(), "a")),
            Box::new(Recorder(log.clone(), "b")),
        ]);
        fanout.handle(&GameEvent::EventEnded {
            event_name: None,
            ctx: crate::events::Context::test_fixture(),
        });
        assert_eq!(*log.lock().unwrap(), ["a:event_ended", "b:event_ended"]);
    }
}
