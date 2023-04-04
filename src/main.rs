mod automation;
mod hass;
mod types;
mod websocket;

use std::sync::Arc;

use automation::EventListener;
use hass::Hass;
use tokio::{self, sync::RwLock};
use types::EventData;

struct TestEvent {}

impl EventListener for TestEvent {
    fn fire(&mut self, event_data: EventData) {
        println!("received event {:?}", event_data);
    }
}

#[tokio::main]
async fn main() {
    let hass = Arc::new(RwLock::new(Hass::new("config.yaml")));

    // run!
    let hass_task = tokio::spawn(hass::start(hass.clone()));

    // add event listeners
    let test_event = TestEvent {};
    {
        let h = hass.write().await;
        h.add_listener(test_event).await;
    }

    hass_task.await.expect("error joining hass_task")
}
