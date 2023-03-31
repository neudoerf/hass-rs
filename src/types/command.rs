use serde::Serialize;

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Auth(Auth),
    SubscribeEvents(SubscribeEvents),
}

#[derive(Serialize, Debug)]
pub struct Auth {
    pub access_token: String,
}

#[derive(Serialize, Debug)]
pub struct SubscribeEvents {
    pub id: u64,
    pub event_type: Option<String>,
}
