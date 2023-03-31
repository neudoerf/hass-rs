use crate::hass::ResponseHandle;

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio::{
    net::TcpStream,
    select,
    sync::mpsc::{self, Receiver},
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::types::{Command, Response};

const API_WEBSOCKET: &str = "/api/websocket";

type WebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(crate) async fn start(url: String, hass_recv: ResponseHandle, cmd_recv: Receiver<Command>) {
    // build the url
    let url_str = format!("{}{}", url, API_WEBSOCKET);
    let url = Url::parse(&url_str).expect(&format!("failed to parse url: {}", url_str));
    // connnect to the server
    let (client, _) = connect_async(url)
        .await
        .expect(&format!("failed to connect to url {}", url_str));
    let (send, recv) = client.split();

    let mut read_task = tokio::spawn(read_loop(recv, hass_recv));
    let mut send_task = tokio::spawn(send_loop(send, cmd_recv));

    select! {
        _ = (&mut read_task) => {
            send_task.abort();
        }
        _ = (&mut send_task) => {
            read_task.abort();
        }
    }
}

async fn read_loop(mut stream: SplitStream<WebSocket>, channel: ResponseHandle) {
    loop {
        match stream.next().await {
            Some(Ok(m)) => match m {
                Message::Text(t) => {
                    let message: Result<Response, serde_json::Error> = serde_json::from_str(&t);
                    match message {
                        Ok(r) => {
                            println!("received message {:?}", r);
                            channel.send(r).await;
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
            Some(Err(error)) => {
                println!("Error {}", error);
            }
            None => {
                break;
            }
        }
    }
}

async fn send_loop(mut send: SplitSink<WebSocket, Message>, mut channel: mpsc::Receiver<Command>) {
    loop {
        match channel.recv().await {
            Some(c) => {
                let message = serde_json::to_string(&c).unwrap();
                send.send(Message::Text(message)).await.unwrap();
            }
            None => {
                break;
            }
        }
        println!("received command");
    }
}
