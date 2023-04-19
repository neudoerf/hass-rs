use std::time::Duration;

use hass_rs::types::{EventData, Target};
use hass_rs::{automation, HassCommand};
use tokio::{self, sync::mpsc, task::JoinHandle};

struct TestEvent {
    hass: mpsc::Sender<HassCommand>,
    handle: Option<JoinHandle<()>>,
    cmd_send: mpsc::Sender<TestEventCommand>,
    cmd_recv: mpsc::Receiver<TestEventCommand>,
    event_send: mpsc::Sender<EventData>,
    event_recv: mpsc::Receiver<EventData>,
}

enum TestEventCommand {
    LightOff,
}

impl TestEvent {
    fn new(hass: mpsc::Sender<HassCommand>) -> Self {
        let (event_send, event_recv) = mpsc::channel(10);
        let (cmd_send, cmd_recv) = mpsc::channel(10);
        Self {
            hass,
            handle: None,
            cmd_send,
            cmd_recv,
            event_send,
            event_recv,
        }
    }

    async fn run(&mut self) {
        let c = HassCommand::SubscribeEvents {
            sender: self.event_send.clone(),
        };
        self.hass.send(c).await.unwrap();
        loop {
            tokio::select! {
                event_data = self.event_recv.recv() => {
                    if let Some(event_data) = event_data {
                        self.event(event_data).await;
                    }
                },
                cmd = self.cmd_recv.recv() => {
                    if let Some(cmd) = cmd {
                        self.command(cmd).await;
                    }
                }
            }
        }
    }

    async fn event(&mut self, event_data: EventData) {
        if event_data.entity_id == "input_boolean.test" {
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
                                entity_id: "light.front_hall".to_owned(),
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
                        entity_id: "light.front_hall".to_owned(),
                    }),
                };
                self.hass.send(c).await.unwrap();
                self.handle = None;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // run!
    let (hass, task) = hass_rs::start("config.yaml").await;

    // add event listeners
    let mut test_event = TestEvent::new(hass.clone());
    let _ = tokio::spawn(async move { test_event.run().await });

    task.await.expect("error joining hass_task")
}
