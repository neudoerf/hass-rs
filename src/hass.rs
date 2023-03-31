use crate::{
    types::{Auth, Command, Response, SubscribeEvents},
    websocket,
};

use std::{fs::File, io::BufReader, sync::Arc};

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
        let f = File::open(config).unwrap();
        let config: HassConfig = serde_yaml::from_reader(BufReader::new(f)).unwrap();
        return config;
    }
}

pub struct Hass {
    config: Arc<HassConfig>,
    sender: Option<Sender<Command>>,
    id: Arc<RwLock<u64>>,
}

impl Hass {
    pub fn new(config: &str) -> Hass {
        Hass {
            config: Arc::new(HassConfig::new(config.to_string())),
            sender: None,
            id: Arc::new(RwLock::new(0)),
        }
    }

    async fn send_auth(&self) {
        let auth = Auth {
            access_token: self.config.access_token.clone(),
        };
        self.send(Command::Auth(auth)).await;
    }

    async fn subscribe_events(&self) {
        let mut id = self.id.write().await;
        *id += 1;
        let sub = SubscribeEvents {
            id: *id,
            event_type: Some("state_changed".to_string()),
        };
        self.send(Command::SubscribeEvents(sub)).await;
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
