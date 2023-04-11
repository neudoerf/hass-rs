use async_trait::async_trait;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::types::EventData;

#[async_trait]
pub trait EventListener {
    async fn handle_event(&mut self, event_data: EventData);
}

struct AutomationActor<T>
where
    T: EventListener + Send,
{
    receiver: Receiver<EventData>,
    handler: T,
}

impl<T> AutomationActor<T>
where
    T: EventListener + Send,
{
    fn new(receiver: Receiver<EventData>, handler: T) -> Self {
        AutomationActor { receiver, handler }
    }

    async fn run(&mut self) {
        while let Some(event_data) = self.receiver.recv().await {
            self.handler.handle_event(event_data).await;
        }
    }
}

pub(crate) struct Automation {
    sender: Sender<EventData>,
}

impl Automation {
    pub(crate) fn new<T: EventListener + Send + 'static>(handler: T) -> Self {
        let (sender, receiver) = mpsc::channel(10);
        let mut actor = AutomationActor::new(receiver, handler);
        tokio::spawn(async move { actor.run().await });
        Automation { sender }
    }

    pub(crate) async fn send(&self, e: EventData) {
        self.sender.send(e).await.unwrap();
    }
}
