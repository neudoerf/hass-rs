use crate::{
    automation::{Automation, EventListener},
    types::{Auth, CallService, Command, EventData, HassEntity, Response, SubscribeEvents, Target},
    websocket,
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
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    RwLock,
};

#[derive(Deserialize)]
struct HassConfig {
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

#[derive(Clone)]
pub struct Hass {
    config: Arc<HassConfig>,
    sender: Option<Sender<Command>>,
    id: Arc<AtomicU64>,
    state: Arc<RwLock<HashMap<String, HassEntity>>>,
    event_listeners: Arc<RwLock<Vec<Automation>>>,
}

impl Hass {
    pub fn new(config: &str) -> Hass {
        Hass {
            config: Arc::new(HassConfig::new(config.to_string())),
            sender: None,
            id: Arc::new(AtomicU64::new(0)),
            state: Arc::new(RwLock::new(HashMap::new())),
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
        if let Some(s) = &self.sender {
            s.send(command)
                .await
                .expect("error sending to command channel");
        }
    }
}

pub async fn start(hass: Arc<RwLock<Hass>>) {
    // create the channels
    let hass_receiver = ResponseHandle::new(hass.clone());
    let (cmd_send, cmd_recv) = mpsc::channel::<Command>(10);
    {
        let mut h = hass.write().await;
        h.sender = Some(cmd_send);
    }

    let h = hass.read().await;
    let ws_task = tokio::spawn(websocket::start(
        h.config.url.clone(),
        hass_receiver,
        cmd_recv,
    ));
    ws_task.await.expect("error joining ws_task");
    println!("websocket closed");
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
