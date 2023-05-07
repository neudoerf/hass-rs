use std::time::Duration;

use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};

use crate::{types::EventData, HassCommand};

pub fn run_in<T: Send + 'static>(
    d: Duration,
    message: T,
    channel: mpsc::Sender<T>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(d).await;
        // if the receiver has been dropped, we don't care about the result
        let _ = channel.send(message).await;
    })
}

pub async fn new<T: Automation + Send + 'static>(
    automation: T,
    cmd_recv: mpsc::Receiver<T::AutomationCommand>,
) -> JoinHandle<()> {
    // first we need to get the event receiver
    let (tx, rx) = oneshot::channel::<broadcast::Receiver<EventData>>();
    let cmd = HassCommand::SubscribeEvents { sender: tx };
    let hass_sender = automation.get_hass();
    // only panics if the receiver has been dropped, which means the hass thread has died
    hass_sender.send(cmd).await.unwrap();
    // wait for Hass to respond with the event receiver
    let event_recv = rx.await.unwrap();

    let mut hass_automation = HassAutomation::new(automation, event_recv, cmd_recv);
    tokio::spawn(async move { hass_automation.run().await })
}

#[async_trait::async_trait]
pub trait Automation {
    type AutomationCommand: Send;

    async fn event(&mut self, event: EventData);

    async fn command(&mut self, command: Self::AutomationCommand);

    fn get_command_channel(&self) -> mpsc::Sender<Self::AutomationCommand>;

    fn get_hass(&self) -> mpsc::Sender<HassCommand>;
}

struct HassAutomation<T>
where
    T: Automation + Send,
{
    automation: T,
    event_recv: broadcast::Receiver<EventData>,
    cmd_recv: mpsc::Receiver<T::AutomationCommand>,
}

impl<T: Automation + Send> HassAutomation<T> {
    fn new(
        automation: T,
        event_recv: broadcast::Receiver<EventData>,
        cmd_recv: mpsc::Receiver<T::AutomationCommand>,
    ) -> Self {
        Self {
            automation,
            event_recv,
            cmd_recv,
        }
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                Ok(event) = self.event_recv.recv() => {
                        self.automation.event(event).await;
                },
                Some(cmd) = self.cmd_recv.recv() => {
                        self.automation.command(cmd).await;
                },
            }
        }
    }
}
