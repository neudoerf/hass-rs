use crate::{
    automation::{Automation, EventListener},
    types::{Auth, CallService, Command, EventData, Response, SubscribeEvents, Target},
    websocket::{self, CommandHandle},
};

use std::{
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use serde::Deserialize;
use tokio::{
    sync::{
        mpsc::{self, Receiver, Sender},
        RwLock,
    },
    task::JoinHandle,
};

const API_WEBSOCKET: &str = "/api/websocket";

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
    config: Arc<HassConfig>,
    cmd_handle: Option<CommandHandle>,
    id: Arc<AtomicU64>,
    event_listeners: Arc<RwLock<Vec<Automation>>>,
}

impl Hass {
    pub fn new(config: HassConfig) -> Hass {
        Hass {
            config: Arc::new(config),
            cmd_handle: None,
            id: Arc::new(AtomicU64::new(0)),
            event_listeners: Arc::new(RwLock::new(Vec::<Automation>::new())),
        }
    }

    pub async fn add_listener<T: EventListener + Send + 'static>(&self, handler: T) {
        let automation = Automation::new(handler);
        let mut listeners = self.event_listeners.write().await;
        listeners.push(automation);
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

    pub async fn call_service(&self, domain: &str, service: &str, target: Option<Target>) {
        let id = self.get_id().await;
        let call_service = CallService {
            id,
            service_type: "call_service".to_string(),
            domain: domain.to_owned(),
            service: service.to_owned(),
            service_data: None,
            target,
        };
        self.send(Command::CallService(call_service)).await;
    }

    async fn process_event(&self, e: EventData) {
        let listeners = self.event_listeners.read().await;
        for listener in listeners.iter() {
            listener.send(e.clone()).await
        }
    }

    async fn send(&self, command: Command) {
        if let Some(cmd_handle) = &self.cmd_handle {
            cmd_handle.send(command).await;
        }
    }
}

pub async fn start(config: &str) -> (Arc<RwLock<Hass>>, JoinHandle<()>) {
    let hass_config = HassConfig::new(config.to_string());
    let hass = Arc::new(RwLock::new(Hass::new(hass_config)));
    let hass_receiver = ResponseHandle::new(hass.clone());
    let cmd_handle;
    let ws_task;
    {
        let h = hass.read().await;
        (cmd_handle, ws_task) = websocket::start(
            format!("{}{}", h.config.url.clone(), API_WEBSOCKET),
            hass_receiver,
        )
        .await
    }
    hass.write().await.cmd_handle = Some(cmd_handle);
    (hass, ws_task)
}

struct ResponseActor {
    receiver: Receiver<Response>,
    hass: Arc<RwLock<Hass>>,
}

impl ResponseActor {
    fn new(receiver: Receiver<Response>, hass: Arc<RwLock<Hass>>) -> Self {
        Self { receiver, hass }
    }

    async fn handle_message(&mut self, response: Response) {
        match response {
            Response::AuthRequired(_) => {
                let h = self.hass.read().await;
                h.send_auth().await;
            }
            Response::AuthOk(_) => {
                let h = self.hass.read().await;
                h.subscribe_events().await;
            }
            Response::Event(e) => {
                let h = self.hass.read().await;
                h.process_event(e.event.data).await;
            }
            _ => {}
        }
    }

    async fn run(&mut self) {
        while let Some(response) = self.receiver.recv().await {
            self.handle_message(response).await;
        }
    }
}

pub(crate) struct ResponseHandle {
    sender: Sender<Response>,
}

impl ResponseHandle {
    pub(crate) fn new(hass: Arc<RwLock<Hass>>) -> Self {
        let (sender, receiver) = mpsc::channel::<Response>(10);
        let mut actor = ResponseActor::new(receiver, hass);
        tokio::spawn(async move { actor.run().await });
        Self { sender }
    }

    pub(crate) async fn send(&self, response: Response) {
        self.sender.send(response).await.unwrap();
    }
}
