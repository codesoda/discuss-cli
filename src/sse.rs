use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BroadcastEvent {
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct EventBus {
    tx: broadcast::Sender<BroadcastEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);

        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: BroadcastEvent) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn event(kind: &str) -> BroadcastEvent {
        BroadcastEvent {
            kind: kind.to_string(),
            payload: json!({
                "threadId": "u-1"
            }),
        }
    }

    #[tokio::test]
    async fn publish_and_subscribe_delivers_event() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let event = event("thread.created");

        bus.publish(event.clone());

        assert_eq!(rx.recv().await.expect("receive event"), event);
    }

    #[tokio::test]
    async fn two_subscribers_each_receive_a_copy() {
        let bus = EventBus::new(16);
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        let event = event("take.added");

        bus.publish(event.clone());

        assert_eq!(first.recv().await.expect("receive first event"), event);
        assert_eq!(second.recv().await.expect("receive second event"), event);
    }

    #[tokio::test]
    async fn dropping_a_subscriber_does_not_panic_publish() {
        let bus = EventBus::new(16);
        let rx = bus.subscribe();

        drop(rx);

        bus.publish(event("reply.added"));
    }
}
