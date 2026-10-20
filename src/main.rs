use std::env;
use std::path::{Path, PathBuf};

use migo_hqm_server::ban::{BanCheck, FileBanCheck, InMemoryBanCheck};
use migo_hqm_server::game::PhysicsConfiguration;
use migo_hqm_server::gamemode::shootout::{ShootoutGameConfiguration, ShootoutGameMode};
use migo_hqm_server::gamemode::standard_match::{
    IcingConfiguration, MatchConfiguration, OffsideConfiguration, OffsideLineConfiguration,
    StandardMatchGameMode, TwoLinePassConfiguration,
};
use migo_hqm_server::gamemode::util::SpawnPoint;
use migo_hqm_server::gamemode::warmup::PermanentWarmup;
use migo_hqm_server::record::{
    RecordingSaveMethod, RecordingSaveToFile, RecordingSendToHttpEndpoint,
};
use migo_hqm_server::{ReplayRecording, ServerConfiguration};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum HQMServerMode {
    Match,
    #[serde(rename = "warmup")]
    PermanentWarmup,
    Shootout,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReplayConfiguration {
    Off,
    On,
    Standby,
}

#[derive(Deserialize)]
struct TomlConfiguration {
    server: ServerSection,
    #[serde(default)]
    game: GameSection,
    #[serde(default)]
    physics: PhysicsSection,
}

#[derive(Deserialize)]
struct ServerSection {
    name: String,
    port: u16,
    public: bool,
    public_address: Option<String>,
    player_max: usize,
    team_max: usize,
    password: Option<String>,
    mode: Option<HQMServerMode>,
    replays: Option<ReplayConfiguration>,
    log_name: Option<String>,
    welcome: Option<String>,
    replay_endpoint: Option<String>,
    replay_directory: Option<PathBuf>,
    service: Option<String>,
    ban_file: Option<PathBuf>,
}

#[derive(Default, Deserialize)]
struct GameSection {
    periods: Option<u32>,
    time_period: Option<u32>,
    time_break: Option<u32>,
    time_intermission: Option<u32>,
    warmup_pucks: Option<usize>,
    attempts: Option<u32>,
    mercy: Option<u32>,
    first: Option<u32>,
    icing: Option<String>,
    offside: Option<String>,
    offsideline: Option<String>,
    twolinepass: Option<String>,
    spawn: Option<String>,
    spawn_offset: Option<f32>,
    spawn_player_altitude: Option<f32>,
    spawn_puck_altitude: Option<f32>,
    spawn_player_keep_stick: Option<bool>,
    use_mph: Option<bool>,
    goal_replay: Option<bool>,
}

#[derive(Default, Deserialize)]
struct PhysicsSection {
    limit_jump_speed: Option<bool>,
    gravity: Option<f32>,
    player_acceleration: Option<f32>,
    player_deceleration: Option<f32>,
    max_player_speed: Option<f32>,
    max_player_shift_speed: Option<f32>,
    puck_rink_friction: Option<f32>,
    player_turning: Option<f32>,
    player_shift_turning: Option<f32>,
    player_shift_acceleration: Option<f32>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    let config_path = if args.len() > 1 {
        &args[1]
    } else {
        "config.toml"
    };

    // Load configuration (if exists)
    if Path::new(config_path).exists() {
        let config_text = std::fs::read_to_string(config_path)?;
        let conf: TomlConfiguration = toml::from_str(&config_text)?;

        // Server information
        let server_section = conf.server;
        let server_name = server_section.name;
        let server_port = server_section.port;
        let server_public = server_section.public;
        let public_address =
            if server_public {
                Some(server_section.public_address.unwrap_or_else(|| {
                    "https://sam2.github.io/HQMMasterServerEndpoint/".to_owned()
                }))
            } else {
                None
            };
        let server_player_max = server_section.player_max;
        let server_team_max = server_section.team_max;

        let server_password = server_section.password;
        let mode = server_section.mode.unwrap_or(HQMServerMode::Match);

        let replays_enabled = match server_section.replays.unwrap_or(ReplayConfiguration::Off) {
            ReplayConfiguration::Off => ReplayRecording::Off,
            ReplayConfiguration::On => ReplayRecording::On,
            ReplayConfiguration::Standby => ReplayRecording::Standby,
        };

        let log_name = server_section
            .log_name
            .unwrap_or_else(|| format!("{server_name}.log"));

        let welcome = server_section.welcome.unwrap_or_default();

        let welcome_str = welcome
            .lines()
            .map(String::from)
            .filter(|x| !x.is_empty())
            .collect();

        let replay_saving: Box<dyn RecordingSaveMethod> =
            if let Some(url) = server_section.replay_endpoint {
                Box::new(RecordingSendToHttpEndpoint::new(url))
            } else {
                let dir = server_section
                    .replay_directory
                    .unwrap_or_else(|| PathBuf::from("replays"));
                Box::new(RecordingSaveToFile::new(dir))
            };

        let server_service = server_section.service;

        let ban_file = server_section.ban_file;

        // Game
        let game_section = conf.game;

        let config = ServerConfiguration {
            welcome: welcome_str,
            password: server_password,
            player_max: server_player_max,
            recording_enabled: replays_enabled,
            server_name,
            server_service,
        };

        // Physics
        let physics_section = conf.physics;
        let limit_jump_speed = physics_section.limit_jump_speed.unwrap_or(false);
        let gravity = physics_section.gravity.unwrap_or(6.80555) / 10000.0;
        let player_acceleration = physics_section.player_acceleration.unwrap_or(2.08333) / 10000.0;
        let player_deceleration = physics_section.player_deceleration.unwrap_or(5.55555) / 10000.0;
        let max_player_speed = physics_section.max_player_speed.unwrap_or(5.0) / 100.0;
        let max_player_shift_speed =
            physics_section.max_player_shift_speed.unwrap_or(3.33333) / 100.0;
        let puck_rink_friction = physics_section.puck_rink_friction.unwrap_or(0.05);
        let player_turning = physics_section.player_turning.unwrap_or(4.1666666) / 10000.0;
        let player_shift_turning =
            physics_section.player_shift_turning.unwrap_or(3.88888) / 10000.0;
        let player_shift_acceleration =
            physics_section.player_shift_acceleration.unwrap_or(2.7777) / 10000.0;

        let physics_config = PhysicsConfiguration {
            gravity,
            limit_jump_speed,
            player_acceleration,
            player_deceleration,
            player_shift_acceleration,
            max_player_speed,
            max_player_shift_speed,
            puck_rink_friction,
            player_turning,
            player_shift_turning,
        };

        let file_appender = tracing_appender::rolling::daily("log", log_name);
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_line_number(false)
            .with_file(false)
            .with_target(false)
            .with_writer(non_blocking)
            .init();

        let ban: Box<dyn BanCheck> = if let Some(ban_file) = ban_file.as_deref() {
            Box::new(FileBanCheck::new(ban_file.to_owned()).await?)
        } else {
            Box::new(InMemoryBanCheck::new())
        };

        match mode {
            HQMServerMode::Match => {
                let periods = game_section.periods.unwrap_or(3);

                let rules_time_period = game_section.time_period.unwrap_or(300);
                let rule_time_break = game_section.time_break.unwrap_or(10);
                let rule_time_intermission = game_section.time_intermission.unwrap_or(20);
                let warmup_pucks = game_section.warmup_pucks.unwrap_or(1);

                let mercy = game_section.mercy.unwrap_or(0);
                let first_to = game_section.first.unwrap_or(0);

                let icing = match game_section.icing.as_deref().unwrap_or("off") {
                    "on" | "touch" => IcingConfiguration::Touch,
                    "notouch" => IcingConfiguration::NoTouch,
                    _ => IcingConfiguration::Off,
                };

                let offside = match game_section.offside.as_deref().unwrap_or("off") {
                    "on" | "delayed" => OffsideConfiguration::Delayed,
                    "immediate" | "imm" => OffsideConfiguration::Immediate,
                    _ => OffsideConfiguration::Off,
                };

                let offside_line = match game_section.offsideline.as_deref().unwrap_or("blue") {
                    "blue" => OffsideLineConfiguration::OffensiveBlue,
                    "center" => OffsideLineConfiguration::Center,
                    _ => OffsideLineConfiguration::OffensiveBlue,
                };

                let twoline_pass = match game_section.twolinepass.as_deref().unwrap_or("off") {
                    "on" => TwoLinePassConfiguration::On,
                    "forward" => TwoLinePassConfiguration::Forward,
                    "double" | "both" => TwoLinePassConfiguration::Double,
                    "blue" | "three" | "threeline" => TwoLinePassConfiguration::ThreeLine,
                    _ => TwoLinePassConfiguration::Off,
                };

                let spawn_point = match game_section.spawn.as_deref().unwrap_or("center") {
                    "bench" => SpawnPoint::Bench,
                    _ => SpawnPoint::Center,
                };

                let spawn_point_offset = game_section.spawn_offset.unwrap_or(2.75);

                let spawn_player_altitude = game_section.spawn_player_altitude.unwrap_or(1.5);

                let spawn_puck_altitude = game_section.spawn_puck_altitude.unwrap_or(1.5);

                let spawn_keep_stick_position =
                    game_section.spawn_player_keep_stick.unwrap_or(false);

                let use_mph = game_section.use_mph.unwrap_or(false);

                let goal_replay = game_section.goal_replay.unwrap_or(false);

                let match_config = MatchConfiguration {
                    time_period: rules_time_period,
                    time_break: rule_time_break,
                    time_intermission: rule_time_intermission,
                    mercy,
                    first_to,
                    icing,
                    offside,
                    offside_line,
                    twoline_pass,
                    warmup_pucks,
                    use_mph,
                    goal_replay,
                    periods,
                    spawn_point_offset,
                    spawn_player_altitude,
                    spawn_puck_altitude,
                    spawn_keep_stick_position,
                };

                migo_hqm_server::run_server(
                    server_port,
                    public_address.as_deref(),
                    config,
                    physics_config,
                    ban,
                    replay_saving,
                    StandardMatchGameMode::new(match_config, server_team_max, spawn_point),
                )
                .await?
            }
            HQMServerMode::PermanentWarmup => {
                let warmup_pucks = game_section.warmup_pucks.unwrap_or(1);

                let spawn_point = match game_section.spawn.as_deref().unwrap_or("center") {
                    "bench" => SpawnPoint::Bench,
                    _ => SpawnPoint::Center,
                };

                migo_hqm_server::run_server(
                    server_port,
                    public_address.as_deref(),
                    config,
                    physics_config,
                    ban,
                    replay_saving,
                    PermanentWarmup::new(warmup_pucks, spawn_point),
                )
                .await?
            }
            HQMServerMode::Shootout => {
                let attempts = game_section.attempts.unwrap_or(5);
                let shootout_config = ShootoutGameConfiguration { attempts };

                migo_hqm_server::run_server(
                    server_port,
                    public_address.as_deref(),
                    config,
                    physics_config,
                    ban,
                    replay_saving,
                    ShootoutGameMode::new(shootout_config),
                )
                .await?;
            }
        };
    } else {
        println!("Could not open configuration file {config_path}!");
    };
    Ok(())
}
