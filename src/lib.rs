pub mod automation;
pub mod types;
mod websocket;

use std::{collections::HashMap, fs::File, io::BufReader};

use serde::Deserialize;
use serde_json::Value;
use tokio::{
    join,
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use types::{
    Auth, CallService, Command, EventData, GetStates, HassEntity, Response, SubscribeEvents, Target,
};
use websocket::CommandHandle;

const API_WEBSOCKET: &str = "/api/websocket";

#[derive(Debug)]
pub enum HassCommand {
    CallService {
        domain: String,
        service: String,
        service_data: Option<Value>,
        target: Option<Target>,
    },
    SubscribeEvents {
        sender: oneshot::Sender<broadcast::Receiver<EventData>>,
    },
    GetState {
        entity_id: String,
        response: oneshot::Sender<Option<HassEntity>>,
    },
    Response(Response),
    SetCommandHandle(CommandHandle),
}

#[derive(Deserialize)]
pub struct HassConfig {
    access_token: String,
    url: String,
}

impl HassConfig {
    pub fn new(config: String) -> HassConfig {
        let f = File::open(config).expect("unable to open config file");
        let config: HassConfig =
            serde_yaml::from_reader(BufReader::new(f)).expect("unable to parse config file");
        return config;
    }
}

pub struct Hass {
    config: HassConfig,
    cmd_handle: Option<CommandHandle>,
    id: u64,
    event_channel: broadcast::Sender<EventData>,
    state: HashMap<String, HassEntity>,
    command_channel: mpsc::Receiver<HassCommand>,
    fetch_states_id: Option<u64>,
}

impl Hass {
    fn new(config: HassConfig, receiver: mpsc::Receiver<HassCommand>) -> Hass {
        let (tx, _) = broadcast::channel(10);
        Hass {
            config,
            cmd_handle: None,
            id: 0,
            event_channel: tx,
            state: HashMap::new(),
            command_channel: receiver,
            fetch_states_id: None,
        }
    }

    fn get_event_channel(&mut self) -> broadcast::Receiver<EventData> {
        self.event_channel.subscribe()
    }

    async fn get_state(&self, entity_id: String) -> Option<HassEntity> {
        self.state.get(&entity_id).cloned()
    }

    async fn run(&mut self) {
        while let Some(command) = self.command_channel.recv().await {
            match command {
                HassCommand::CallService {
                    domain,
                    service,
                    service_data,
                    target,
                } => {
                    self.call_service(&domain, &service, service_data, target)
                        .await;
                }
                HassCommand::SubscribeEvents { sender } => {
                    // don't care if sending fails
                    let _ = sender.send(self.get_event_channel());
                }
                HassCommand::GetState {
                    entity_id,
                    response,
                } => {
                    let state = self.get_state(entity_id).await;
                    println!("Sending state: {:#?}", state);
                    response.send(state).unwrap();
                }
                HassCommand::Response(response) => {
                    self.process_response(response).await;
                }
                HassCommand::SetCommandHandle(handle) => {
                    self.cmd_handle = Some(handle);
                }
            }
        }
    }

    async fn process_response(&mut self, response: Response) {
        match response {
            Response::Event(data) => {
                if data.event.event_type == "state_changed" {
                    let entity_id = data.event.data.entity_id.clone();
                    let new_state = data.event.data.new_state.clone();
                    if let Some(new_state) = new_state {
                        self.state.insert(entity_id, new_state);
                    }
                }
                // only fails if there are no receivers, which we don't care about
                let _ = self.event_channel.send(data.event.data);
            }
            Response::AuthRequired(_) => {
                println!("Got auth required");
                self.send_auth().await;
            }
            Response::AuthOk(_) => {
                println!("Got auth ok");
                // subscribe to the event stream
                self.subscribe_events().await;
                // fetch all current states
                self.fetch_states_id = Some(self.fetch_states().await);
            }
            Response::AuthInvalid(_) => {
                println!("Got auth invalid");
            }
            Response::Result(result) => match self.fetch_states_id {
                Some(id) if id == result.id => {
                    match result.result {
                        Some(Value::Array(states)) => {
                            for state in states {
                                if let Ok(state) =
                                    serde_json::from_value::<HassEntity>(state.clone())
                                {
                                    self.state.insert(state.entity_id.clone(), state);
                                } else {
                                    println!("Unable to parse state: {:#?}", state);
                                }
                            }
                        }
                        _ => {}
                    }
                    self.fetch_states_id = None;
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn get_id(&mut self) -> u64 {
        self.id += 1;
        self.id
    }

    async fn send_auth(&self) {
        let auth = Auth {
            access_token: self.config.access_token.clone(),
        };
        self.send(Command::Auth(auth)).await;
    }

    async fn subscribe_events(&mut self) {
        let id = self.get_id();
        let sub = SubscribeEvents {
            id,
            event_type: Some("state_changed".to_string()),
        };
        self.send(Command::SubscribeEvents(sub)).await;
    }

    async fn fetch_states(&mut self) -> u64 {
        let id = self.get_id();
        let get_state = GetStates {
            id,
            command_type: "get_states".to_string(),
        };
        self.send(Command::GetStates(get_state)).await;
        id
    }

    pub async fn call_service(
        &mut self,
        domain: &str,
        service: &str,
        service_data: Option<Value>,
        target: Option<Target>,
    ) {
        let id = self.get_id();
        let call_service = CallService {
            id,
            service_type: "call_service".to_string(),
            domain: domain.to_owned(),
            service: service.to_owned(),
            service_data,
            target,
        };
        self.send(Command::CallService(call_service)).await;
    }

    async fn send(&self, command: Command) {
        if let Some(cmd_handle) = &self.cmd_handle {
            cmd_handle.send(command).await;
        }
    }
}

pub async fn start(config: &str) -> (mpsc::Sender<HassCommand>, JoinHandle<()>) {
    let hass_config = HassConfig::new(config.to_string());
    let (hass_sender, hass_receiver) = mpsc::channel(5);
    let response_handle = ResponseHandle::new(hass_sender.clone());
    let ws_task = tokio::spawn(websocket::start(
        format!("{}{}", hass_config.url.clone(), API_WEBSOCKET),
        response_handle,
        hass_sender.clone(),
    ));
    let mut hass = Hass::new(hass_config, hass_receiver);
    let hass_task = tokio::spawn(async move { hass.run().await });
    let task = tokio::spawn(async move {
        let _ = join!(hass_task, ws_task);
    });
    (hass_sender, task)
}

struct ResponseActor {
    receiver: mpsc::Receiver<Response>,
    sender: mpsc::Sender<HassCommand>,
}

impl ResponseActor {
    fn new(receiver: mpsc::Receiver<Response>, sender: mpsc::Sender<HassCommand>) -> Self {
        Self { receiver, sender }
    }

    async fn handle_message(&mut self, response: Response) {
        let _ = self.sender.send(HassCommand::Response(response)).await;
    }

    async fn run(&mut self) {
        while let Some(response) = self.receiver.recv().await {
            self.handle_message(response).await;
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResponseHandle {
    sender: mpsc::Sender<Response>,
}

impl ResponseHandle {
    pub(crate) fn new(cmd_send: mpsc::Sender<HassCommand>) -> Self {
        let (sender, receiver) = mpsc::channel::<Response>(10);
        let mut actor = ResponseActor::new(receiver, cmd_send);
        tokio::spawn(async move { actor.run().await });
        Self { sender }
    }

    pub(crate) async fn send(&self, response: Response) {
        self.sender.send(response).await.unwrap();
    }
}
