use std::time::Duration;

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio::{
    net::TcpStream,
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::{
    types::{Command, Response},
    HassCommand, ResponseHandle,
};

type WebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct SendActor {
    sender: SplitSink<WebSocket, Message>,
    receiver: Receiver<Command>,
}

impl SendActor {
    fn new(sender: SplitSink<WebSocket, Message>, receiver: Receiver<Command>) -> Self {
        Self { sender, receiver }
    }

    async fn run(&mut self) {
        loop {
            match timeout(Duration::from_secs(8), self.receiver.recv()).await {
                Ok(Some(command)) => {
                    let message = serde_json::to_string(&command).unwrap();
                    self.sender.send(Message::Text(message)).await.unwrap();
                }
                Ok(None) => {
                    println!("Connection closed");
                    break;
                }
                Err(_) => {
                    self.sender.send(Message::Ping(vec![])).await.unwrap();
                }
            }
        }
    }
}

struct ReceiveActor {
    receiver: SplitStream<WebSocket>,
    channel: ResponseHandle,
}

impl ReceiveActor {
    pub(crate) fn new(receiver: SplitStream<WebSocket>, channel: ResponseHandle) -> Self {
        Self { receiver, channel }
    }

    async fn run(&mut self) {
        loop {
            match timeout(Duration::from_secs(18), self.receiver.next()).await {
                Ok(Some(message)) => match message {
                    Ok(m) => match m {
                        Message::Text(t) => {
                            let message: Result<Response, serde_json::Error> =
                                serde_json::from_str(&t);
                            match message {
                                Ok(r) => {
                                    // println!("received message {:?}", r);
                                    self.channel.send(r).await;
                                }
                                Err(e) => {
                                    println!("Failed to deserialize: {}\nError: {}", t, e)
                                }
                            }
                        }
                        Message::Ping(_) => {
                            println!("received ping");
                        }
                        Message::Pong(_) => {
                            println!("received pong");
                        }
                        Message::Close(_) => {
                            println!("connection closed by server");
                            break;
                        }
                        _ => {
                            println!("Non-text message recieved: {:?}", m)
                        }
                    },
                    Err(e) => {
                        println!("Error: {}", e);
                        break;
                    }
                },
                Ok(None) => {
                    println!("Connection closed");
                    break;
                }
                Err(_) => {
                    println!("Timeout");
                    break;
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommandHandle {
    sender: Sender<Command>,
}

impl CommandHandle {
    fn new(ws: WebSocket, resp_handle: ResponseHandle) -> (Self, JoinHandle<()>) {
        let (ws_send, ws_recv) = ws.split();
        let (cmd_send, cmd_recv) = mpsc::channel::<Command>(10);
        let send_actor = SendActor::new(ws_send, cmd_recv);
        let recv_actor = ReceiveActor::new(ws_recv, resp_handle);

        let handle = tokio::spawn(async move { spawn(send_actor, recv_actor).await });
        (Self { sender: cmd_send }, handle)
    }

    pub(crate) async fn send(&self, command: Command) {
        self.sender.send(command).await.unwrap();
    }
}

async fn spawn(mut send_actor: SendActor, mut recv_actor: ReceiveActor) {
    let mut send_handle = tokio::spawn(async move { send_actor.run().await });
    let mut recv_handle = tokio::spawn(async move { recv_actor.run().await });

    tokio::select! {
        _ = &mut send_handle => {
            recv_handle.abort();
        }
        _ = &mut recv_handle => {
            send_handle.abort();
        }
    }
}

pub(crate) async fn start(
    url_str: String,
    hass_recv: ResponseHandle,
    hass_sender: Sender<HassCommand>,
) {
    // build the url
    let url = Url::parse(&url_str).expect(&format!("failed to parse url: {}", url_str));
    loop {
        // connnect to the server
        if let Ok((client, _)) = connect_async(url.clone()).await {
            println!("Connected to {}", url_str);
            let (handle, task) = CommandHandle::new(client, hass_recv.clone());
            hass_sender
                .send(HassCommand::SetCommandHandle(handle))
                .await
                .unwrap();
            task.await.unwrap();
        } else {
            println!("Failed to connect to {}", url_str);
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}
