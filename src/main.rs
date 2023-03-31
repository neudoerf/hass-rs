mod hass;
mod types;
mod websocket;

use std::sync::Arc;

use hass::Hass;
use tokio::{self, sync::RwLock};

#[tokio::main]
async fn main() {
    let hass = Arc::new(RwLock::new(Hass::new("config.yaml")));

    // run!
    let hass_task = tokio::spawn(hass::start(hass.clone()));

    hass_task.await.expect("error joining hass_task")
}
