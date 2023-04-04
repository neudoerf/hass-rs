mod automation;
mod hass;
mod types;
mod websocket;

use std::sync::Arc;

use automation::EventListener;
use hass::Hass;
use tokio::{self, sync::RwLock};
use types::EventData;

use crate::types::Target;

struct TestEvent {
    hass: Arc<RwLock<Hass>>,
}

#[async_trait::async_trait]
impl EventListener for TestEvent {
    async fn handle_event(&mut self, event_data: EventData) {
        println!("received event {:?}", event_data);
        if event_data.entity_id == "input_boolean.test" {
            if let Some(state) = event_data.new_state {
                let h = self.hass.read().await;
                let service = if state.state == "on" {
                    "turn_on"
                } else {
                    "turn_off"
                };
                h.call_service(
                    "light",
                    service,
                    Some(Target {
                        entity_id: "light.front_hall".to_owned(),
                    }),
                )
                .await;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let hass = Arc::new(RwLock::new(Hass::new("config.yaml")));

    // run!
    let hass_task = tokio::spawn(hass::start(hass.clone()));

    // add event listeners
    let test_event = TestEvent { hass: hass.clone() };
    {
        let h = hass.read().await;
        h.add_listener(test_event).await;
    }

    hass_task.await.expect("error joining hass_task")
}
