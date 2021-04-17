use std::path::Path;

// INI Crate For configuration
extern crate ini;
use ini::Ini;
use std::env;
use crate::hqm_server::{HQMServerConfiguration, HQMIcingConfiguration, HQMOffsideConfiguration, HQMServerMode, HQMSpawnPoint, HQMMatchConfiguration};

mod hqm_parse;
mod hqm_simulate;
mod hqm_game;
mod hqm_server;
mod hqm_admin_commands;
mod hqm_rules;

use tracing_subscriber;
use tracing_appender;
use ini::ini::Properties;

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

        fn get_optional <U, F: FnOnce(&str) -> U>(section: Option<&Properties>, property: &str, default: U, f: F) -> U {
            section.and_then( |x| x.get(property)).map_or(default, f)
        }

        // Game
        let game_section = conf.section(Some("Game"));

        let rules_time_period = get_optional(game_section, "time_period", 300, |x| x.parse::<u32>().unwrap());
        let rules_time_warmup = get_optional(game_section, "time_warmup", 300, |x| x.parse::<u32>().unwrap());
        let rule_time_break = get_optional(game_section, "time_break", 10, |x| x.parse::<u32>().unwrap());
        let rule_time_intermission = get_optional(game_section, "time_intermission", 20, |x| x.parse::<u32>().unwrap());
        let warmup_pucks = get_optional(game_section, "warmup_pucks", 1, |x| x.parse::<usize>().unwrap());

        let limit_jump_speed = get_optional(game_section, "limit_jump_speed", false, |x| x.eq_ignore_ascii_case("true"));

        let mercy = get_optional(game_section, "mercy", 0, |x| x.parse::<u32>().unwrap());
        let first_to = get_optional(game_section, "first", 0, |x| x.parse::<u32>().unwrap());

        let icing = get_optional(game_section, "icing", HQMIcingConfiguration::Off, |x| match x {
            "on" | "touch" => HQMIcingConfiguration::Touch,
            "notouch" => HQMIcingConfiguration::NoTouch,
            _ => HQMIcingConfiguration::Off
        });

        let offside = get_optional(game_section, "offside", HQMOffsideConfiguration::Off, |x| match x {
            "on" | "delayed" => HQMOffsideConfiguration::Delayed,
            "immediate" | "imm" => HQMOffsideConfiguration::Immediate,
            _ => HQMOffsideConfiguration::Off
        });

        let spawn_point = get_optional(game_section, "spawn", HQMSpawnPoint::Center, |x| match x {
            "bench" => HQMSpawnPoint::Bench,
            _ => HQMSpawnPoint::Center
        });

        let config = HQMServerConfiguration {
            welcome: welcome_str,
            password: server_password,
            player_max: server_player_max,
            replays_enabled,
            server_name
        };

        let match_config = HQMMatchConfiguration {
            team_max: server_team_max,

            time_period: rules_time_period, 
            time_warmup: rules_time_warmup, 
            time_break: rule_time_break,
            time_intermission: rule_time_intermission,
            mercy,
            first_to,
            icing,
            offside,
            warmup_pucks,
            force_team_size_parity,
            limit_jump_speed,
            cheats_enabled,
            spawn_point,
            mode,
        };

        let file_appender = tracing_appender::rolling::daily("log", log_name);
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_writer(non_blocking)
            .init();

        // Config file didn't exist; use defaults as described
        return hqm_server::run_server(server_port, server_public, config, match_config).await;
    } else {
        println! ("Could not open configuration file {}!", config_path);
        return Ok(())
    };

}

