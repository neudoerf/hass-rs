use std::time::Duration;

use hass_rs::automation::Automation;
use hass_rs::types::{EventData, Target};
use hass_rs::{automation, HassCommand};
use tokio::{self, sync::mpsc, task::JoinHandle};

struct TestEvent {
    hass: mpsc::Sender<HassCommand>,
    cmd_send: mpsc::Sender<TestEventCommand>,
    trigger: String,
    entity: String,
    handle: Option<JoinHandle<()>>,
}

enum TestEventCommand {
    LightOff,
}

impl TestEvent {
    async fn start(
        hass: mpsc::Sender<HassCommand>,
        trigger: String,
        entity: String,
    ) -> JoinHandle<()> {
        let (cmd_send, cmd_recv) = mpsc::channel(1);
        automation::new(
            TestEvent {
                hass,
                cmd_send,
                trigger,
                entity,
                handle: None,
            },
            cmd_recv,
        )
        .await
    }
}

#[async_trait::async_trait]
impl Automation for TestEvent {
    type AutomationCommand = TestEventCommand;

    async fn event(&mut self, event_data: EventData) {
        if event_data.entity_id == self.trigger {
            if let Some(state) = event_data.new_state {
                if state.state == "on" {
                    if let Some(handle) = &self.handle {
                        handle.abort();
                        self.handle = None;
                    } else {
                        let c = HassCommand::CallService {
                            domain: "light".to_owned(),
                            service: "turn_on".to_owned(),
                            service_data: None,
                            target: Some(Target {
                                entity_id: self.entity.clone(),
                            }),
                        };
                        self.hass.send(c).await.unwrap();
                    }
                } else {
                    self.handle = Some(automation::run_in(
                        Duration::from_secs(10),
                        TestEventCommand::LightOff,
                        self.cmd_send.clone(),
                    ));
                };
            }
        }
    }

    async fn command(&mut self, cmd: TestEventCommand) {
        match cmd {
            TestEventCommand::LightOff => {
                let c = HassCommand::CallService {
                    domain: "light".to_owned(),
                    service: "turn_off".to_owned(),
                    service_data: None,
                    target: Some(Target {
                        entity_id: self.entity.clone(),
                    }),
                };
                self.hass.send(c).await.unwrap();
                self.handle = None;
            }
        }
    }

    fn get_command_channel(&self) -> mpsc::Sender<TestEventCommand> {
        self.cmd_send.clone()
    }

    fn get_hass(&self) -> mpsc::Sender<HassCommand> {
        self.hass.clone()
    }
}

#[tokio::main]
async fn main() {
    // run!
    let (hass, task) = hass_rs::start("config.yaml").await;

    // add event listeners
    TestEvent::start(
        hass.clone(),
        "input_boolean.test".to_owned(),
        "light.front_hall".to_owned(),
    )
    .await;

    task.await.expect("error joining hass_task")
}
