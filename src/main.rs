use std::path::Path;

// INI Crate For configuration
extern crate ini;
use ini::Ini;
use std::env;

mod hqm_match;

mod hqm_russian;
mod hqm_shootout;
mod hqm_warmup;

use crate::hqm_match::HQMMatchBehaviour;

use crate::hqm_russian::HQMRussianBehaviour;
use crate::hqm_shootout::HQMShootoutBehaviour;
use crate::hqm_warmup::HQMPermanentWarmup;
use ini::Properties;
use migo_hqm_server::hqm_game::HQMPhysicsConfiguration;
use migo_hqm_server::hqm_match_util::{
    HQMIcingConfiguration, HQMMatchConfiguration, HQMOffsideConfiguration,
    HQMOffsideLineConfiguration, HQMSpawnPoint, HQMTwoLinePassConfiguration,
};
use migo_hqm_server::hqm_server;
use migo_hqm_server::hqm_server::{HQMServerConfiguration, ReplayEnabled, ReplaySaving};
use tracing_appender;
use tracing_subscriber;

enum HQMServerMode {
    Match,
    PermanentWarmup,
    Russian,
    Shootout,
}

fn is_true(s: &str) -> bool {
    s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("on")
}

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
        let server_name = server_section
            .get("name")
            .unwrap()
            .parse::<String>()
            .unwrap();
        let server_port = server_section.get("port").unwrap().parse::<u16>().unwrap();
        let server_public = is_true(server_section.get("public").unwrap());
        let public_address = if server_public {
            Some(
                server_section
                    .get("public_address")
                    .unwrap_or("https://sam2.github.io/HQMMasterServerEndpoint/"),
            )
        } else {
            None
        };
        let server_player_max = server_section
            .get("player_max")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let server_team_max = server_section
            .get("team_max")
            .unwrap()
            .parse::<usize>()
            .unwrap();

        let server_password = server_section
            .get("password")
            .unwrap()
            .parse::<String>()
            .unwrap();
        let mode = server_section
            .get("mode")
            .map_or(HQMServerMode::Match, |x| match x {
                "warmup" => HQMServerMode::PermanentWarmup,
                "match" => HQMServerMode::Match,
                "russian" => HQMServerMode::Russian,
                "shootout" => HQMServerMode::Shootout,
                _ => HQMServerMode::Match,
            });

        let replays_enabled = match server_section.get("replays") {
            Some(s) if is_true(s) => ReplayEnabled::On,
            Some(s) if s.eq_ignore_ascii_case("standby") => ReplayEnabled::Standby,
            _ => ReplayEnabled::Off,
        };

        let log_name = server_section
            .get("log_name")
            .map_or(format!("{}.log", server_name), |x| String::from(x));

        let welcome = server_section.get("welcome").unwrap_or("");

        let welcome_str = welcome
            .lines()
            .map(String::from)
            .filter(|x| !x.is_empty())
            .collect();

        let replay_saving =
            server_section
                .get("replay_endpoint")
                .map_or(ReplaySaving::File, |url| ReplaySaving::Endpoint {
                    url: url.to_string(),
                });

        fn get_optional<U, F: FnOnce(&str) -> U>(
            section: Option<&Properties>,
            property: &str,
            default: U,
            f: F,
        ) -> U {
            section.and_then(|x| x.get(property)).map_or(default, f)
        }

        let server_service = server_section.get("service").map(|x| x.to_owned());

        // Game
        let game_section = conf.section(Some("Game"));

        let limit_jump_speed = get_optional(game_section, "limit_jump_speed", false, is_true);

        let config = HQMServerConfiguration {
            welcome: welcome_str,
            password: server_password,
            player_max: server_player_max,
            replays_enabled,
            replay_saving,
            server_name,
            server_service,
        };

        // Physics
        let physics_section = conf.section(Some("Physics"));
        let gravity = get_optional(physics_section, "gravity", 0.000680555, |x| {
            x.parse::<f32>().unwrap() / 10000.0
        });
        let player_acceleration =
            get_optional(physics_section, "player_acceleration", 0.000208333, |x| {
                x.parse::<f32>().unwrap() / 10000.0
            });
        let player_deceleration =
            get_optional(physics_section, "player_deceleration", 0.000555555, |x| {
                x.parse::<f32>().unwrap() / 10000.0
            });
        let max_player_speed = get_optional(physics_section, "max_player_speed", 0.05, |x| {
            x.parse::<f32>().unwrap() / 100.0
        });
        let max_player_shift_speed =
            get_optional(physics_section, "max_player_shift_speed", 0.0333333, |x| {
                x.parse::<f32>().unwrap() / 100.0
            });

        let puck_rink_friction = get_optional(physics_section, "puck_rink_friction", 0.05, |x| {
            x.parse::<f32>().unwrap()
        });
        let player_turning = get_optional(physics_section, "player_turning", 0.00041666666, |x| {
            x.parse::<f32>().unwrap() / 10000.0
        });
        let player_shift_turning = get_optional(
            physics_section,
            "player_shift_turning",
            0.00038888888,
            |x| x.parse::<f32>().unwrap() / 10000.0,
        );

        let player_shift_acceleration = get_optional(
            physics_section,
            "player_shift_acceleration",
            0.00027777,
            |x| x.parse::<f32>().unwrap() / 10000.0,
        );

        let physics_config = HQMPhysicsConfiguration {
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

        return match mode {
            HQMServerMode::Match => {
                let periods =
                    get_optional(game_section, "periods", 3, |x| x.parse::<u32>().unwrap());

                let rules_time_period = get_optional(game_section, "time_period", 300, |x| {
                    x.parse::<u32>().unwrap()
                });
                let rules_time_warmup = get_optional(game_section, "time_warmup", 300, |x| {
                    x.parse::<u32>().unwrap()
                });
                let rule_time_break = get_optional(game_section, "time_break", 10, |x| {
                    x.parse::<u32>().unwrap()
                });
                let rule_time_intermission =
                    get_optional(game_section, "time_intermission", 20, |x| {
                        x.parse::<u32>().unwrap()
                    });
                let warmup_pucks = get_optional(game_section, "warmup_pucks", 1, |x| {
                    x.parse::<usize>().unwrap()
                });

                let mercy = get_optional(game_section, "mercy", 0, |x| x.parse::<u32>().unwrap());
                let first_to =
                    get_optional(game_section, "first", 0, |x| x.parse::<u32>().unwrap());

                let icing =
                    get_optional(
                        game_section,
                        "icing",
                        HQMIcingConfiguration::Off,
                        |x| match x {
                            "on" | "touch" => HQMIcingConfiguration::Touch,
                            "notouch" => HQMIcingConfiguration::NoTouch,
                            _ => HQMIcingConfiguration::Off,
                        },
                    );

                let offside = get_optional(
                    game_section,
                    "offside",
                    HQMOffsideConfiguration::Off,
                    |x| match x {
                        "on" | "delayed" => HQMOffsideConfiguration::Delayed,
                        "immediate" | "imm" => HQMOffsideConfiguration::Immediate,
                        _ => HQMOffsideConfiguration::Off,
                    },
                );

                let offside_line = get_optional(
                    game_section,
                    "offsideline",
                    HQMOffsideLineConfiguration::OffensiveBlue,
                    |x| match x {
                        "blue" => HQMOffsideLineConfiguration::OffensiveBlue,
                        "center" => HQMOffsideLineConfiguration::Center,
                        _ => HQMOffsideLineConfiguration::OffensiveBlue,
                    },
                );

                let twoline_pass = get_optional(
                    game_section,
                    "twolinepass",
                    HQMTwoLinePassConfiguration::Off,
                    |x| match x {
                        "on" => HQMTwoLinePassConfiguration::On,
                        "forward" => HQMTwoLinePassConfiguration::Forward,
                        "double" | "both" => HQMTwoLinePassConfiguration::Double,
                        "blue" | "three" | "threeline" => HQMTwoLinePassConfiguration::ThreeLine,
                        _ => HQMTwoLinePassConfiguration::Off,
                    },
                );

                let spawn_point =
                    get_optional(game_section, "spawn", HQMSpawnPoint::Center, |x| match x {
                        "bench" => HQMSpawnPoint::Bench,
                        _ => HQMSpawnPoint::Center,
                    });

                let spawn_point_offset =
                    get_optional(game_section, "spawn_offset", 2.75f32, |x| x.parse::<f32>().unwrap());

                let spawn_player_altitude = get_optional(game_section, "spawn_player_altitude", 1.5f32, |x| x.parse::<f32>().unwrap());

                let spawn_puck_altitude = get_optional(game_section, "spawn_puck_altitude", 1.5f32, |x| x.parse::<f32>().unwrap());

                let use_mph = get_optional(game_section, "use_mph", false, is_true);

                let goal_replay = get_optional(game_section, "goal_replay", false, is_true);

                let match_config = HQMMatchConfiguration {
                    time_period: rules_time_period,
                    time_warmup: rules_time_warmup,
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
                    physics_config,
                    periods,
                    spawn_point_offset,
                    spawn_player_altitude,
                    spawn_puck_altitude
                };

                hqm_server::run_server(
                    server_port,
                    public_address,
                    config,
                    HQMMatchBehaviour::new(match_config, server_team_max, spawn_point),
                )
                .await
            }
            HQMServerMode::PermanentWarmup => {
                let warmup_pucks = get_optional(game_section, "warmup_pucks", 1, |x| {
                    x.parse::<usize>().unwrap()
                });

                let spawn_point =
                    get_optional(game_section, "spawn", HQMSpawnPoint::Center, |x| match x {
                        "bench" => HQMSpawnPoint::Bench,
                        _ => HQMSpawnPoint::Center,
                    });

                hqm_server::run_server(
                    server_port,
                    public_address,
                    config,
                    HQMPermanentWarmup::new(physics_config, warmup_pucks, spawn_point),
                )
                .await
            }
            HQMServerMode::Russian => {
                let attempts =
                    get_optional(game_section, "attempts", 10, |x| x.parse::<u32>().unwrap());

                hqm_server::run_server(
                    server_port,
                    public_address,
                    config,
                    HQMRussianBehaviour::new(attempts, server_team_max, physics_config),
                )
                .await
            }
            HQMServerMode::Shootout => {
                let attempts =
                    get_optional(game_section, "attempts", 5, |x| x.parse::<u32>().unwrap());

                hqm_server::run_server(
                    server_port,
                    public_address,
                    config,
                    HQMShootoutBehaviour::new(attempts, physics_config),
                )
                .await
            }
        };
    } else {
        println!("Could not open configuration file {}!", config_path);
        return Ok(());
    };
}
