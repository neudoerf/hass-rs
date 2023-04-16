mod hass;
mod types;
mod websocket;

use hass::HassCommand;
use tokio::{self, sync::mpsc};

use crate::types::Target;

struct TestEvent {
    hass: mpsc::Sender<HassCommand>,
}

impl TestEvent {
    fn new(hass: mpsc::Sender<HassCommand>) -> Self {
        Self { hass }
    }

    async fn run(&self) {
        let (event_send, mut event_recv) = mpsc::channel(10);
        let c = HassCommand::SubscribeEvents { sender: event_send };
        self.hass.send(c).await.unwrap();
        while let Some(event_data) = event_recv.recv().await {
            if event_data.entity_id == "input_boolean.test" {
                if let Some(state) = event_data.new_state {
                    let service = if state.state == "on" {
                        "turn_on"
                    } else {
                        "turn_off"
                    };
                    let c = HassCommand::CallService {
                        domain: "light".to_owned(),
                        service: service.to_owned(),
                        service_data: None,
                        target: Some(Target {
                            entity_id: "light.front_hall".to_owned(),
                        }),
                    };
                    self.hass.send(c).await.unwrap();
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // run!
    let (hass, task) = hass::start("config.yaml").await;

    // add event listeners
    let test_event = TestEvent::new(hass.clone());
    let _ = tokio::spawn(async move { test_event.run().await });

    task.await.expect("error joining hass_task")
}
