use std::path::Path;

// INI Crate For configuration
extern crate ini;
use ini::Ini;
use std::env;
use crate::hqm_server::{HQMServer, HQMServerConfiguration, HQMIcingConfiguration, HQMOffsideConfiguration, HQMServerMode, HQMSpawnPoint};
use crate::hqm_game::HQMPhysicsConfig;

mod hqm_parse;
mod hqm_simulate;
mod hqm_game;
mod hqm_server;
mod hqm_admin_commands;

use tracing_subscriber;
use tracing_appender;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = env::args().collect();

    let config_path = if args.len() > 1 {
        &args[1]
    } else {
        "config.ini"
    };

    // Load configuration (if exists)
    if Path::new(config_path).exists() {

        // Load configuration file
        let conf = Ini::load_from_file(config_path).unwrap();

        // Server information
        let server_section = conf.section(Some("Server")).unwrap();
        let server_name = server_section.get("name").unwrap().parse::<String>().unwrap();
        let server_port = server_section.get("port").unwrap().parse::<u16>().unwrap();
        let server_public = server_section.get("public").unwrap().parse::<bool>().unwrap();
        let server_player_max = server_section.get("player_max").unwrap().parse::<usize>().unwrap();
        let server_team_max = server_section.get("team_max").unwrap().parse::<usize>().unwrap();
        let force_team_size_parity = match server_section.get("force_team_size_parity") {
            Some(s) => s.eq_ignore_ascii_case("true"),
            None => false
        };
        let server_password = server_section.get("password").unwrap().parse::<String>().unwrap();
        let mode = server_section.get("mode").map_or(HQMServerMode::Match, |x| {
            match x {
                "warmup" => HQMServerMode::PermanentWarmup,
                "match" => HQMServerMode::Match,
                _ => HQMServerMode::Match
            }
        });

        let replays_enabled = match server_section.get("replays") {
            Some(s) => s.eq_ignore_ascii_case("true"),
            None => false
        };

        let cheats_enabled = match server_section.get("cheats_enabled") {
            Some(s) => s.eq_ignore_ascii_case("true"),
            None => false
        };
        let log_name = server_section.get("log_name").map_or(format!("{}.log", server_name) , |x| String::from(x));

        let welcome = server_section.get("welcome").unwrap_or("");

        let welcome_str = welcome.lines()
            .map(String::from)
            .filter(|x| !x.is_empty()).collect();

        // Game
        let game_section = conf.section(Some("Game")).unwrap();

        let rules_time_period = game_section.get("time_period").map_or(300, |x| x.parse::<u32>().unwrap());
        let rules_time_warmup = game_section.get("time_warmup").map_or(300, |x| x.parse::<u32>().unwrap());
        let rule_time_break = game_section.get("time_break").map_or(10, |x| x.parse::<u32>().unwrap());
        let rule_time_intermission = game_section.get("time_intermission").map_or(20, |x| x.parse::<u32>().unwrap());
        let warmup_pucks = game_section.get("warmup_pucks").map_or_else(|| 1, |x| x.parse::<usize>().unwrap());

        let limit_jump_speed = match game_section.get("limit_jump_speed") {
            Some(s) => s.eq_ignore_ascii_case("true"),
            None => false
        };

        let cylinder_puck_post_collision = match game_section.get("cylinder_puck_post_collision") {
            Some(s) => s.eq_ignore_ascii_case("true"),
            None => false
        };

        let icing = game_section.get("icing").map_or(HQMIcingConfiguration::Off, |x| match x {
            "on" | "touch" => HQMIcingConfiguration::Touch,
            "notouch" => HQMIcingConfiguration::NoTouch,
            _ => HQMIcingConfiguration::Off
        });

        let offside = game_section.get("offside").map_or(HQMOffsideConfiguration::Off, |x| match x {
            "on" | "delayed" => HQMOffsideConfiguration::Delayed,
            "immediate" | "imm" => HQMOffsideConfiguration::Immediate,
            _ => HQMOffsideConfiguration::Off
        });

        let spawn_point = game_section.get("spawn").map_or(HQMSpawnPoint::Center, |x| match x {
            "bench" => HQMSpawnPoint::Bench,
            _ => HQMSpawnPoint::Center
        });

        // Physics
        let physics_section = conf.section(Some("Physics"));

        let player_acceleration: f32 = physics_section.and_then( |x| x.get("player_acceleration")).map_or(0.000208333f32, |x| x.parse::<f32>().unwrap());
        let player_deceleration: f32 = physics_section.and_then( |x| x.get("player_deceleration")).map_or(0.000555555f32, |x| x.parse::<f32>().unwrap());
        let puck_to_ice_linear_friction: f32 = physics_section.and_then( |x| x.get("puck_to_ice_linear_friction")).map_or(0.05, |x| x.parse::<f32>().unwrap());
        let gravity: f32 = physics_section.and_then( |x| x.get("gravity")).map_or(0.000680555f32, |x| x.parse::<f32>().unwrap());

        let config = HQMServerConfiguration {
            server_name,
            port: server_port,
            team_max: server_team_max,
            player_max: server_player_max,
            public: server_public,

            password: server_password,

            time_period: rules_time_period,
            time_warmup: rules_time_warmup,
            time_break: rule_time_break,
            time_intermission: rule_time_intermission,
            icing,
            offside,
            warmup_pucks,
            force_team_size_parity,
            cheats_enabled,
            replays_enabled,
            spawn_point,
            cylinder_puck_post_collision,

            welcome: welcome_str,
            mode,
            physics_configuration: HQMPhysicsConfig {
                player_acceleration,
                player_deceleration,
                puck_to_ice_linear_friction,
                gravity,
                limit_jump_speed
            }

        };

        let file_appender = tracing_appender::rolling::daily("log", log_name);
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_writer(non_blocking)
            .init();

        // Config file didn't exist; use defaults as described
        return HQMServer::new(config).run().await;
    } else {
        println! ("Could not open configuration file {}!", config_path);
        return Ok(())
    };

}

