mod admin_commands;

pub mod gamemode;

pub mod ban;
pub mod game;
pub mod physics;
mod protocol;
pub mod record;
mod server;

pub use server::run_server;

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
