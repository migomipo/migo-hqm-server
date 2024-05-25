mod admin_commands;

pub mod gamemode;

pub mod ban;
pub mod game;
pub mod physics;
mod protocol;
mod server;

pub use server::run_server;

#[derive(Debug, Clone)]
pub enum ReplaySaving {
    File,
    Endpoint { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ReplayEnabled {
    Off,
    On,
    Standby,
}

#[derive(Debug, Clone)]
pub struct ServerConfiguration {
    pub welcome: Vec<String>,
    pub password: Option<String>,
    pub player_max: usize,

    pub replays_enabled: ReplayEnabled,
    pub replay_saving: ReplaySaving,
    pub server_name: String,
    pub server_service: Option<String>,
}
