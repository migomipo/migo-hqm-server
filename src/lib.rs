pub mod gamemode;

pub mod game;
pub mod physics;
mod protocol;
mod server;

pub(crate) use server::players;
pub use server::{ban, recording as record, run_server};

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ReplayRecording {
    Off,
    On,
    Standby,
}

#[derive(Debug, Clone)]
pub struct ServerConfiguration {
    pub welcome: Vec<String>,
    pub password: Option<String>,
    pub player_max: usize,

    pub recording_enabled: ReplayRecording,
    pub server_name: String,
    pub server_service: Option<String>,
}
