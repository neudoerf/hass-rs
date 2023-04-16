use crate::{
    types::{Auth, CallService, Command, EventData, HassEntity, Response, SubscribeEvents, Target},
    websocket::{self, CommandHandle},
};

use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use serde::Deserialize;
use serde_json::Value;
use tokio::{
    join,
    sync::{mpsc, oneshot, RwLock},
    task::JoinHandle,
};

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
        sender: mpsc::Sender<EventData>,
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
    id: Arc<AtomicU64>,
    event_subscribers: Arc<RwLock<Vec<mpsc::Sender<EventData>>>>,
    state: Arc<RwLock<HashMap<String, HassEntity>>>,
    command_channel: mpsc::Receiver<HassCommand>,
}

impl Hass {
    fn new(config: HassConfig, receiver: mpsc::Receiver<HassCommand>) -> Hass {
        Hass {
            config,
            cmd_handle: None,
            id: Arc::new(AtomicU64::new(0)),
            event_subscribers: Arc::new(RwLock::new(Vec::new())),
            state: Arc::new(RwLock::new(HashMap::new())),
            command_channel: receiver,
        }
    }

    async fn add_subscriber(&self, sender: mpsc::Sender<EventData>) {
        let mut subscribers = self.event_subscribers.write().await;
        subscribers.push(sender);
    }

    async fn get_state(&self, entity_id: String) -> Option<HassEntity> {
        let state = self.state.read().await;
        state.get(&entity_id).cloned()
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
                    self.add_subscriber(sender).await;
                }
                HassCommand::GetState {
                    entity_id,
                    response,
                } => {
                    let state = self.get_state(entity_id).await;
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

    async fn process_response(&self, response: Response) {
        match response {
            Response::Event(data) => {
                if data.event.event_type == "state_changed" {
                    let mut state = self.state.write().await;
                    let entity_id = data.event.data.entity_id.clone();
                    let new_state = data.event.data.new_state.clone();
                    if let Some(new_state) = new_state {
                        state.insert(entity_id, new_state);
                    }
                }
                self.process_event(data.event.data).await;
            }
            Response::AuthRequired(_) => {
                println!("Got auth required");
                self.send_auth().await;
            }
            Response::AuthOk(_) => {
                println!("Got auth ok");
                self.subscribe_events().await;
            }
            Response::AuthInvalid(_) => {
                println!("Got auth invalid");
            }
            _ => {}
        }
    }

    async fn get_id(&self) -> u64 {
        let mut id = self.id.load(Ordering::Relaxed);
        id += 1;
        self.id.store(id, Ordering::Release);
        id
    }

    async fn send_auth(&self) {
        let auth = Auth {
            access_token: self.config.access_token.clone(),
        };
        self.send(Command::Auth(auth)).await;
    }

    async fn subscribe_events(&self) {
        let id = self.get_id().await;
        let sub = SubscribeEvents {
            id,
            event_type: Some("state_changed".to_string()),
        };
        self.send(Command::SubscribeEvents(sub)).await;
    }

    pub async fn call_service(
        &self,
        domain: &str,
        service: &str,
        service_data: Option<Value>,
        target: Option<Target>,
    ) {
        let id = self.get_id().await;
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

    async fn process_event(&self, e: EventData) {
        let subscribers = self.event_subscribers.read().await;
        for subscriber in subscribers.iter() {
            let _ = subscriber.send(e.clone()).await.unwrap();
        }
    }

    async fn send(&self, command: Command) {
        if let Some(cmd_handle) = &self.cmd_handle {
            cmd_handle.send(command).await;
        }
    }
}

pub async fn start(config: &str) -> (mpsc::Sender<HassCommand>, JoinHandle<()>) {
    let hass_config = HassConfig::new(config.to_string());
    let (hass_sender, hass_receiver) = mpsc::channel(10);
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
