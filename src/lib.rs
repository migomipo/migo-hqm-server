pub mod gamemode;

pub mod game;
mod history;
pub mod physics;
mod protocol;
mod server;

pub(crate) use server::players;
pub use server::{ServerExit, ban, recording as record, run_server};

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
    pub restart: RestartMethod,
}

/// How the /serverrestart command restarts the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartMethod {
    /// The server shuts down and exits with [EXIT_CODE_RESTART], and a supervisor such as systemd
    /// or Docker starts it again.
    Exit,
    /// /serverrestart is not available.
    Unavailable,
}

/// The exit code after /serverrestart with [RestartMethod::Exit], which supervisors should treat
/// as a request to start the server again. It is `EX_TEMPFAIL` from sysexits.h.
pub const EXIT_CODE_RESTART: u8 = 75;

/// The exit code when the configuration could not be loaded, which supervisors should not
/// restart the server for. It is `EX_CONFIG` from sysexits.h.
pub const EXIT_CODE_CONFIG: u8 = 78;
