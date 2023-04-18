use std::time::Duration;

use tokio::{sync::mpsc, task::JoinHandle};

pub fn run_in<T: Send + 'static>(
    d: Duration,
    message: T,
    channel: mpsc::Sender<T>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(d).await;
        let _ = channel.send(message).await;
    })
}

