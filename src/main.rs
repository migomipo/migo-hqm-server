use nalgebra::Vector3;
use std::path::Path;

// INI Crate For configuration
extern crate ini;
use ini::Ini;
use std::env;
use crate::hqm_server::{HQMServer, HQMServerConfiguration, HQMIcingConfiguration, HQMOffsideConfiguration};

mod hqm_parse;
mod hqm_simulate;
mod hqm_game;
mod hqm_server;
mod hqm_admin_commands;

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
        let server_player_max = server_section.get("player_max").unwrap().parse::<u32>().unwrap();
        let server_team_max = server_section.get("team_max").unwrap().parse::<u32>().unwrap();
        let force_team_size_parity = match server_section.get("force_team_size_parity") {
            Some(s) => s.eq_ignore_ascii_case("true"),
            None => false
        };
        let server_password = server_section.get("password").unwrap().parse::<String>().unwrap();

        let welcome = server_section.get("welcome").unwrap_or("");

        let welcome_str = welcome.lines()
            .map(String::from)
            .filter(|x| !x.is_empty()).collect();

        // Rules
        let rules_section = conf.section(Some("Rules")).unwrap();
        let rules_time_period = rules_section.get("time_period").unwrap().parse::<u32>().unwrap();
        let rules_time_warmup = rules_section.get("time_warmup").unwrap().parse::<u32>().unwrap();
        let rules_time_intermission = rules_section.get("time_intermission").unwrap().parse::<u32>().unwrap();
        let warmup_pucks = rules_section.get("warmup_pucks").map_or_else(|| 1, |x| x.parse::<u32>().unwrap());

        // Game
        let game_section = conf.section(Some("Game")).unwrap();

        // Game: Red Entry Offset
        let mut red_game_entry_offset:Vector3<f32>=Vector3::new(15.0,2.75,27.75);
        let red_entry_offset_parts = game_section.get("entry_point_red").unwrap().parse::<String>().unwrap();
        let red_offset_parts: Vec<&str> = red_entry_offset_parts.split(',').collect();

        red_game_entry_offset[0] = red_offset_parts[0].parse::<f32>().unwrap();
        red_game_entry_offset[1] = red_offset_parts[1].parse::<f32>().unwrap();
        red_game_entry_offset[2] = red_offset_parts[2].parse::<f32>().unwrap();
        let red_game_entry_rotation = red_offset_parts[3].parse::<f32>().unwrap() * (std::f32::consts::PI/180.0);

        // Game: Blue Entry Offset
        let mut blue_game_entry_offset:Vector3<f32>=Vector3::new(15.0,2.75,33.25);
        let blue_entry_offset_parts = game_section.get("entry_point_blue").unwrap().parse::<String>().unwrap();
        let blue_offset_parts: Vec<&str> = blue_entry_offset_parts.split(',').collect();

        blue_game_entry_offset[0] = blue_offset_parts[0].parse::<f32>().unwrap();
        blue_game_entry_offset[1] = blue_offset_parts[1].parse::<f32>().unwrap();
        blue_game_entry_offset[2] = blue_offset_parts[2].parse::<f32>().unwrap();
        let blue_game_entry_rotation = blue_offset_parts[3].parse::<f32>().unwrap() * (std::f32::consts::PI/180.0);

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


        let config = HQMServerConfiguration {
            server_name,
            port: server_port,
            team_max: server_team_max,
            player_max: server_player_max,
            public: server_public,

            password: server_password,

            time_period: rules_time_period, 
            time_warmup: rules_time_warmup, 
            time_intermission: rules_time_intermission,
            icing,
            offside,
            warmup_pucks,
            force_team_size_parity,
            limit_jump_speed,
            cylinder_puck_post_collision,

            entry_point_red:red_game_entry_offset,
            entry_rotation_red:red_game_entry_rotation,

            entry_point_blue:blue_game_entry_offset,
            entry_rotation_blue:blue_game_entry_rotation,

            welcome: welcome_str,
        };
        // Config file didn't exist; use defaults as described
        return HQMServer::new(config).run().await;
    } else {
        println! ("Could not open configuration file {}!", config_path);
        return Ok(())
    };

}

