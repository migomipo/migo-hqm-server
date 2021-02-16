use std::net::{SocketAddr};

use nalgebra::{Vector3, Point3, Matrix3, Vector2, Rotation3};

use std::cmp::min;
use std::time::{Duration, Instant};

use crate::hqm_parse::{HQMMessageReader, HQMMessageWriter, HQMObjectPacket};
use crate::hqm_simulate::HQMSimulationEvent;
use crate::hqm_game::{HQMTeam, HQMGameObject, HQMGameState, HQMSkaterHand, HQMGameWorld, HQMMessage, HQMGame, HQMPlayerInput, HQMIcingStatus, HQMOffsideStatus, HQMRulesState, HQMPhysicsConfig};
use tokio::net::UdpSocket;
use std::rc::Rc;
use std::collections::{HashSet, HashMap};
use std::sync::Arc;
use bytes::{BytesMut, Bytes};

use tracing::info;
use std::collections::VecDeque;
use std::f32::consts::{PI, FRAC_PI_2};

use std::net::IpAddr;
use std::error::Error;
use tokio::io::AsyncWriteExt;
use tokio::fs::File;
use std::path::PathBuf;

const GAME_HEADER: &[u8] = b"Hock";

pub struct HQMSavedTick {
    packets: Vec<HQMObjectPacket>,
    time: Instant,
}

enum HQMServerReceivedData {
    GameClientPacket {
        addr: SocketAddr,
        data: Bytes,
    }
}

pub(crate) struct HQMServer {
    pub(crate) players: Vec<Option<HQMConnectedPlayer>>,
    pub(crate) ban_list: HashSet<std::net::IpAddr>,
    pub(crate) allow_join: bool,
    pub(crate) config: HQMServerConfiguration,
    pub(crate) game: HQMGame,
    game_alloc: u32,
    pub(crate) is_muted:bool,
}

impl HQMServer {
    async fn handle_message(&mut self, addr: SocketAddr, socket: & Arc<UdpSocket>, msg: &[u8]) {
        let mut parser = HQMMessageReader::new(&msg);
        let header = parser.read_bytes_aligned(4);
        if header != GAME_HEADER {
            return;
        }

        let command = parser.read_byte_aligned();
        match command {
            0 => {
                self.request_info(socket, addr, &mut parser);
            },
            2 => {
                self.player_join(addr, &mut parser);
            },
            // if 8 or 0x10, client is modded, probly want to send it to the player_update function to store it in the client/player struct, to use when responding to clients
            4 | 8 | 0x10 => {
                self.player_update(addr, &mut parser, command);
            },
            7 => {
                self.player_exit(addr);
            },
            _ => {}
        }
    }

    fn request_info<'a>(&self, socket: & Arc<UdpSocket>, addr: SocketAddr, parser: &mut HQMMessageReader<'a>) {
        let mut write_buf = vec![0u8;512];
        let _player_version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();

        let mut writer = HQMMessageWriter::new(& mut write_buf);
        writer.write_bytes_aligned(GAME_HEADER);
        writer.write_byte_aligned(1);
        writer.write_bits(8, 55);
        writer.write_u32_aligned(ping);

        let player_count  = self.player_count();
        writer.write_bits(8, player_count as u32);
        writer.write_bits(4, 4);
        writer.write_bits(4, self.config.team_max as u32);

        writer.write_bytes_aligned_padded(32, self.config.server_name.as_ref());

        let written = writer.get_bytes_written();
        let socket = socket.clone();
        let addr = addr.clone();
        tokio::spawn(async move {
            let slice= &write_buf[0..written];
            let _ = socket.send_to(slice, addr).await;
        });

    }

    fn player_count (& self) -> usize {
        let mut player_count = 0;
        for player in &self.players {
            if player.is_some() {
                player_count += 1;
            }
        }
        player_count
    }

    fn player_update(&mut self, addr: SocketAddr, parser: &mut HQMMessageReader, command: u8) {
        let current_slot = self.find_player_slot(addr);
        let (player_index, player) = match current_slot {
            Some(x) => {
                (x, self.players[x].as_mut().unwrap())
            }
            None => {
                return;
            }
        };

        // Set client version based on the command used to trigger player_update
        // Huge thank you to Baba for his help with this!
        match command {
            4 => {
                player.client_version = 0; // Cryptic
            },
            8 => {
                player.client_version = 1; // Baba - Ping
            },
            0x10 => {
                player.client_version = 2; // Baba - Ping + Rules
            },
            _ => {}
        }

        let current_game_id = parser.read_u32_aligned();

        let input_stick_angle = parser.read_f32_aligned();
        let input_turn = parser.read_f32_aligned();
        let input_unknown = parser.read_f32_aligned();
        let input_fwbw = parser.read_f32_aligned();
        let input_stick_rot_1 = parser.read_f32_aligned();
        let input_stick_rot_2 = parser.read_f32_aligned();
        let input_head_rot = parser.read_f32_aligned();
        let input_body_rot = parser.read_f32_aligned();
        let input_keys = parser.read_u32_aligned();
        let input = HQMPlayerInput {
            stick_angle: input_stick_angle,
            turn: input_turn,
            unknown: input_unknown,
            fwbw: input_fwbw,
            stick: Vector2::new (input_stick_rot_1, input_stick_rot_2),
            head_rot: input_head_rot,
            body_rot: input_body_rot,
            keys: input_keys,
        };

        // if modded client get deltatime
        if player.client_version > 0 {
            let delta = parser.read_u32_aligned();
            player.deltatime = delta;
        }

        let packet = parser.read_u32_aligned();

        if player.game_id == current_game_id && player.known_packet < packet {
            if let Some(diff) = self.game.packet.checked_sub(packet) {
                let diff = diff as usize;
                let t1 = Instant::now();
                if let Some (t2) = self.game.saved_ticks.get(diff).map(|x| x.time) {
                    if let Some(duration) = t1.checked_duration_since(t2) {
                        player.last_ping.truncate(100 - 1);
                        player.last_ping.push_front(duration.as_secs_f32());
                    }
                }
            }
        }

        player.inactivity = 0;
        player.known_packet = packet;
        player.input = input;
        player.game_id = current_game_id;
        player.known_msgpos = parser.read_u16_aligned() as usize;

        let has_chat_msg = parser.read_bits(1) == 1;
        if has_chat_msg {
            let rep = parser.read_bits(3) as u8;
            if player.chat_rep != Some(rep) {
                player.chat_rep = Some(rep);
                let byte_num = parser.read_bits(8) as usize;
                let message = parser.read_bytes_aligned(byte_num);
                self.process_message(message, player_index);
            }

        }
    }

    fn player_join(&mut self, addr: SocketAddr, parser: &mut HQMMessageReader) {
        let player_count = self.player_count();
        let max_player_count = self.config.player_max;
        if player_count >= max_player_count {
            return; // Ignore join request
        }
        let player_version = parser.read_bits(8);
        if player_version != 55 {
            return; // Not the right version
        }
        let current_slot = self.find_player_slot( addr);
        if current_slot.is_some() {
            return; // Player has already joined
        }

        // Check ban list
        if self.ban_list.contains(&addr.ip()){
            return;
        }

        // Disabled join
        if !self.allow_join{
            return;
        }

        let player_name_bytes = parser.read_bytes_aligned(32);
        let player_name = get_player_name(player_name_bytes);
        match player_name {
            Some(name) => {
                if let Some(player_index) = self.add_player(name.clone(), addr) {
                    info!("{} ({}) joined server from address {:?}", name, player_index, addr);
                    let msg = format!("{} joined", name);
                    self.add_server_chat_message(msg);
                }
            }
            _ => {}
        };
    }


    fn set_hand (& mut self, hand: HQMSkaterHand, player_index: usize) {
        if let Some(player) = & mut self.players[player_index] {
            player.hand = hand;
            if let Some(skater_obj_index) = player.skater {
                if let HQMGameObject::Player(skater) = & mut self.game.world.objects[skater_obj_index] {
                    if self.game.state == HQMGameState::Game {
                        let msg = format!("Stick hand will change after next intermission");
                        self.add_directed_server_chat_message(msg, player_index);

                        return;
                    }

                    skater.hand = hand;
                }
            }
        }
    }

    fn process_command (&mut self, command: &str, arg: &str, player_index: usize) {

        match command{
            "enablejoin" => {
                self.set_allow_join(player_index,true);
            },
            "disablejoin" => {
                self.set_allow_join(player_index,false);
            },
            "mute" => {
                if let Ok(mute_player_index) = arg.parse::<usize>() {
                    if mute_player_index < self.players.len() {
                        self.mute_player(player_index, mute_player_index);
                    }
                }
            },
            "unmute" => {
                if let Ok(mute_player_index) = arg.parse::<usize>() {
                    if mute_player_index < self.players.len() {
                        self.unmute_player(player_index, mute_player_index);
                    }
                }
            },
            /*"shadowmute" => {
                if let Ok(mute_player_index) = arg.parse::<usize>() {
                    if mute_player_index < self.players.len() {
                        self.shadowmute_player(player_index, mute_player_index);
                    }
                }
            },*/
            "mutechat" => {
                self.mute_chat(player_index);
            },
            "unmutechat" => {
                self.unmute_chat(player_index);
            },
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<usize>() {
                    if force_player_index < self.players.len() {
                        self.force_player_off_ice(player_index, force_player_index);
                    }
                }
            },
            "kick" => {
                if let Ok(kick_player_index) = arg.parse::<usize>() {
                    if kick_player_index < self.players.len() {
                        self.kick_player(player_index, kick_player_index, false);
                    }
                }
            },
            "kickall" => {
                self.kick_all_matching(player_index, arg,false);
            },
            "ban" => {
                if let Ok(kick_player_index) = arg.parse::<usize>() {
                    if kick_player_index < self.players.len() {
                        self.kick_player(player_index, kick_player_index, true);
                    }
                }
            },
            "banall" => {
                self.kick_all_matching(player_index, arg,true);
            },
            "clearbans" => {
                self.clear_bans(player_index);
            },
            "set" => {
                let args = arg.split(" ").collect::<Vec<&str>>();
                if args.len() > 1{
                    match args[0]{
                        "redscore" =>{

                            let input_score = match args[1].parse::<i32>() {
                                Ok(input_score) => input_score,
                                Err(_) => -1
                            };

                            if input_score >= 0{
                                self.set_score(HQMTeam::Red,input_score as u32,player_index)
                            }
                        },
                        "bluescore" =>{
                            let input_score = match args[1].parse::<i32>() {
                                Ok(input_score) => input_score,
                                Err(_) => -1
                            };

                            if input_score >= 0{
                                self.set_score(HQMTeam::Blue,input_score as u32,player_index)
                            }
                        },
                        "period" =>{
                            let input_period = match args[1].parse::<i32>() {
                                Ok(input_period) => input_period,
                                Err(_) => -1
                            };

                            if input_period >= 0{
                                self.set_period(input_period as u32,player_index)
                            }
                        },
                        "clock" =>{

                            let time_part_string = match args[1].parse::<String>(){
                                Ok(time_part_string) => time_part_string,
                                Err(_) => {return;}
                            };

                            let time_parts: Vec<&str> = time_part_string.split(':').collect();

                            if time_parts.len() >= 2{
                                let time_minutes = match time_parts[0].parse::<i32>() {
                                    Ok(time_minutes) => time_minutes,
                                    Err(_) => -1
                                };

                                let time_seconds = match time_parts[1].parse::<i32>() {
                                    Ok(time_seconds) => time_seconds,
                                    Err(_) => -1
                                };

                                if time_minutes < 0 || time_seconds < 0{
                                    return;
                                }

                                self.set_clock(time_minutes as u32,time_seconds as u32, player_index);
                            }
                        },
                        "hand" =>{
                            match args[1]{
                                "left" =>{
                                    self.set_hand(HQMSkaterHand::Left, player_index);
                                },
                                "right" =>{
                                    self.set_hand(HQMSkaterHand::Right, player_index);
                                },
                                _=>{}
                            }
                        },
                        "icing" => {
                            if let Some(arg) = args.get(1) {
                                self.set_icing_rule(player_index, arg);
                            }
                        },
                        "offside" => {
                            if let Some(arg) = args.get(1) {
                                self.set_offside_rule(player_index, arg);
                            }
                        },
                        "mercy" => {
                            if let Some(arg) = args.get(1) {
                                self.set_mercy_rule(player_index, arg);
                            }
                        },
                        "first" => {
                            if let Some(arg) = args.get(1) {
                                self.set_first_to_rule(player_index, arg);
                            }
                        }
                        "teamsize" => {
                            if let Some(arg) = args.get(1) {
                                self.set_team_size(player_index, arg);
                            }
                        },
                        "teamparity" => {
                            if let Some(arg) = args.get(1) {
                                self.set_team_parity(player_index, arg);
                            }
                        },
                        "replay" => {
                            if let Some(arg) = args.get(1) {
                                self.set_replay(player_index, arg);
                            }
                        }
                        _ => {}
                    }
                }
            },
            "sp" | "setposition" => {
                self.set_preferred_faceoff_position(player_index, arg);
            },
            "admin" => {
                self.admin_login(player_index,arg);
            },
            "faceoff" => {
                self.faceoff(player_index);
            },
            "start" | "startgame" => {
                self.start_game(player_index);
            },
            "reset" | "resetgame" => {
                self.reset_game(player_index);
            },
            "pause" | "pausegame" => {
                self.pause(player_index);
            },
            "unpause" | "unpausegame" => {
                self.unpause(player_index);
            },
            "lefty" => {
                self.set_hand(HQMSkaterHand::Left, player_index);
            },
            "righty" => {
                self.set_hand(HQMSkaterHand::Right, player_index);
            },
            "list" => {
                if arg.is_empty() {
                    self.list_players(player_index, 0);
                } else if let Ok(first_index) = arg.parse::<usize>() {
                    self.list_players(player_index, first_index);
                }
            },
            "search" => {
                self.search_players(player_index, arg);
            },
            "view" => {
                if let Ok(view_player_index) = arg.parse::<usize>() {
                    self.view(view_player_index, player_index);
                }
            },
            "restoreview" => {
                if let Some(player) = & mut self.players[player_index] {
                    if player.view_player_index != player_index {
                        player.view_player_index = player_index;
                        self.add_directed_server_chat_message("View has been restored".to_string(), player_index);
                    }
                }
            },
            "ping" => {
                if let Ok(ping_player_index) = arg.parse::<usize>() {
                    self.ping(ping_player_index, player_index);
                }
            },
            "pings" => {
                if let Some((ping_player_index, _name)) = self.player_exact_unique_match(arg) {
                    self.ping(ping_player_index, player_index);
                } else {
                    let matches = self.player_search(arg);
                    if matches.is_empty() {
                        self.add_directed_server_chat_message("No matches found".to_string(), player_index);
                    } else if matches.len() > 1 {
                        self.add_directed_server_chat_message("Multiple matches found, use /ping X".to_string(), player_index);
                        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
                            self.add_directed_server_chat_message(format!("{}: {}", found_player_index, found_player_name), player_index);
                        }
                    } else {
                        self.ping(matches[0].0, player_index);
                    }
                }
            },
            "views" => {
                if let Some((view_player_index, _name)) = self.player_exact_unique_match(arg) {
                    self.view(view_player_index, player_index);
                } else {
                    let matches = self.player_search(arg);
                    if matches.is_empty() {
                        self.add_directed_server_chat_message("No matches found".to_string(), player_index);
                    } else if matches.len() > 1 {
                        self.add_directed_server_chat_message("Multiple matches found, use /view X".to_string(), player_index);
                        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
                            self.add_directed_server_chat_message(format!("{}: {}", found_player_index, found_player_name), player_index);
                        }
                    } else {
                        self.view(matches[0].0, player_index);
                    }
                }
            }
            "icing" => {
                self.set_icing_rule (player_index, arg);
            },
            "offside" => {
                self.set_offside_rule (player_index, arg);
            },
            "rules" => {
                let offside_str = match self.config.offside {
                    HQMOffsideConfiguration::Off => "Offside disabled",
                    HQMOffsideConfiguration::Delayed => "Offside enabled",
                    HQMOffsideConfiguration::Immediate => "Immediate offside enabled"
                };
                let icing_str = match self.config.icing {
                    HQMIcingConfiguration::Off => "Icing disabled",
                    HQMIcingConfiguration::Touch => "Icing enabled",
                    HQMIcingConfiguration::NoTouch => "No-touch icing enabled"
                };
                let msg = format!("{}, {}", offside_str, icing_str);
                self.add_directed_server_chat_message(msg, player_index);
            },
            "cheat" => {
                if self.config.cheats_enabled {
                    self.cheat(player_index, arg);
                }
            },
            /*
            "test" => {
                let rink = &self.game.world.rink;
                let faceoff_spot = match arg {
                    "c" => Some(rink.center_faceoff_spot.clone()),
                    "r1" => Some(rink.red_zone_faceoff_spots[0].clone()),
                    "r2" => Some(rink.red_zone_faceoff_spots[1].clone()),
                    "b1" => Some(rink.blue_zone_faceoff_spots[0].clone()),
                    "b2" => Some(rink.blue_zone_faceoff_spots[1].clone()),
                    "rn1" => Some(rink.red_neutral_faceoff_spots[0].clone()),
                    "rn2" => Some(rink.red_neutral_faceoff_spots[1].clone()),
                    "bn1" => Some(rink.blue_neutral_faceoff_spots[0].clone()),
                    "bn2" => Some(rink.blue_neutral_faceoff_spots[1].clone()),
                    _ => None
                };
                if let Some(faceoff_spot) = faceoff_spot {
                    self.game.next_faceoff_spot = faceoff_spot;
                    self.do_faceoff();
                }
            }
            */

            _ => {}, // matches have to be exhaustive
        }

    }

    fn list_players (& mut self, player_index: usize, first_index: usize) {
        let mut found = vec![];
        for player_index in first_index..self.players.len() {
            if let Some(player) = & self.players[player_index] {
                found.push((player_index, player.player_name.clone()));
                if found.len() >= 5 {
                    break;
                }
            }
        }
        for (found_player_index, found_player_name) in found {
            self.add_directed_server_chat_message(format!("{}: {}", found_player_index, found_player_name), player_index);
        }
    }

    fn search_players (& mut self, player_index: usize, name: &str) {
        let matches = self.player_search(name);
        if matches.is_empty() {
            self.add_directed_server_chat_message("No matches found".to_string(), player_index);
            return;
        }
        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
            self.add_directed_server_chat_message(format!("{}: {}", found_player_index, found_player_name), player_index);
        }
    }

    fn view (& mut self, view_player_index: usize, player_index: usize) {
        if view_player_index < self.players.len() {
            if let Some(view_player) = &self.players[view_player_index] {
                let view_player_name = view_player.player_name.clone();
                if let Some(player) = &mut self.players[player_index] {
                    if view_player_index != player.view_player_index {
                        player.view_player_index = view_player_index;
                        if player_index != view_player_index {
                            if set_team_internal(player_index, player, &mut self.game.world, &self.config, None).is_some() {
                                let msg = HQMMessage::PlayerUpdate {
                                    player_name: player.player_name.clone(),
                                    object: None,
                                    player_index,
                                    in_server: true
                                };
                                self.add_global_message(msg, true);
                            };
                            self.add_directed_server_chat_message(format!("You are now viewing {}", view_player_name), player_index);
                        } else {
                            self.add_directed_server_chat_message("View has been restored".to_string(), player_index);
                        }
                    }
                }
            } else {
                self.add_directed_server_chat_message("No player with this ID exists".to_string(), player_index);
            }
        }
    }

    fn ping (& mut self, ping_player_index: usize, player_index: usize) {
        if ping_player_index < self.players.len() {
            if let Some(ping_player) = & self.players[ping_player_index] {
                if ping_player.last_ping.is_empty() {
                    let msg = format!("No ping values found for {}", ping_player.player_name);
                    self.add_directed_server_chat_message(msg, player_index);
                } else {
                    let n = ping_player.last_ping.len() as f32;
                    let mut min = f32::INFINITY;
                    let mut max = f32::NEG_INFINITY;
                    let mut sum = 0f32;
                    for i in ping_player.last_ping.iter() {
                        min = min.min(*i);
                        max = max.max(*i);
                        sum += *i;
                    }
                    let avg = sum / n;
                    let dev = {
                        let mut s = 0f32;
                        for i in ping_player.last_ping.iter() {
                            s += (*i - avg).powi(2);
                        }
                        (s / n).sqrt()
                    };

                    let msg1 = format!("{} ping: avg {:.0} ms", ping_player.player_name, (avg * 1000f32));
                    let msg2 = format!("min {:.0} ms, max {:.0} ms, std.dev {:.1}", (min * 1000f32), (max * 1000f32), (dev * 1000f32));
                    self.add_directed_server_chat_message(msg1, player_index);
                    self.add_directed_server_chat_message(msg2, player_index);
                }
            } else {
                self.add_directed_server_chat_message("No player with this ID exists".to_string(), player_index);
            }
        }
    }

    pub(crate) fn player_exact_unique_match(&self, name: &str) -> Option<(usize, String)> {
        let mut found = None;
        for (player_index, player) in self.players.iter ().enumerate() {
            if let Some(player) = player {
                if player.player_name == name {
                    if found.is_none() {
                        found = Some((player_index, player.player_name.clone()));
                    } else {
                        return None
                    }
                }
            }
        }
        found
    }

    pub(crate) fn player_search(&self, name: &str) -> Vec<(usize, String)> {
        let name = name.to_lowercase();
        let mut found = vec![];
        for (player_index, player) in self.players.iter ().enumerate() {
            if let Some(player) = player {
                if player.player_name.to_lowercase().contains(&name) {
                    found.push((player_index, player.player_name.clone()));
                    if found.len() >= 5 {
                        break;
                    }
                }
            }
        }
        found
    }

    fn process_message(&mut self, bytes: Vec<u8>, player_index: usize) {
        let msg = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return
        };

        if self.players[player_index].is_some() {
            if msg.starts_with("/") {
                let split: Vec<&str> = msg.splitn(2, " ").collect();
                let command = &split[0][1..];
                let arg = if split.len() < 2 {
                    ""
                } else {
                    &split[1]
                };
                self.process_command(command, arg, player_index);
            } else {
                if !self.is_muted {
                    match &self.players[player_index as usize] {
                        Some(player) => {
                            match player.is_muted {
                                HQMMuteStatus::NotMuted => {
                                    self.add_user_chat_message(msg, player_index);
                                }
                                HQMMuteStatus::ShadowMuted => {
                                    self.add_directed_user_chat_message(msg, player_index, player_index);
                                }
                                HQMMuteStatus::Muted => {}
                            }
                        },
                        _=>{return;}
                    }
                }
            }
        }
    }

    fn player_exit(&mut self, addr: SocketAddr) {
        let player_index = self.find_player_slot(addr);
        match player_index {
            Some(player_index) => {
                let player_name = {
                    let player = self.players[player_index].as_ref().unwrap();
                    player.player_name.clone()
                };
                self.remove_player(player_index);
                info!("{} ({}) exited server", player_name, player_index);
                let msg = format!("{} exited", player_name);
                self.add_server_chat_message(msg);
            }
            None => {

            }
        }
    }


    #[allow(dead_code)]
    pub(crate) fn set_team (& mut self, player_index: usize, team: Option<HQMTeam>) -> Option<Option<(usize, HQMTeam)>> {
        match & mut self.players[player_index as usize] {
            Some(player) => {
                let res = set_team_internal(player_index, player, & mut self.game.world, & self.config, team);
                if let Some(object) = res {
                    let msg = HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone(),
                        object,
                        player_index,
                        in_server: true
                    };
                    self.add_global_message(msg, true);
                }
                res
            }
            None => { None }
        }
    }

    fn add_player(&mut self, player_name: String, addr: SocketAddr) -> Option<usize> {
        let player_index = self.find_empty_player_slot();
        match player_index {
            Some(player_index) => {
                let update = HQMMessage::PlayerUpdate {
                    player_name: player_name.clone(),
                    object: None,
                    player_index,
                    in_server: true,
                };

                self.add_global_message(update, true);

                let mut messages = self.game.persistent_messages.clone();
                for welcome_msg in self.config.welcome.iter() {
                    messages.push(Rc::new(HQMMessage::Chat {
                        player_index: None,
                        message: welcome_msg.clone()
                    }));
                }

                let new_player = HQMConnectedPlayer::new(player_index, player_name, addr, messages);

                self.players[player_index] = Some(new_player);

                Some(player_index)
            }
            _ => None
        }
    }

    pub(crate) fn remove_player(&mut self, player_index: usize) {

        let mut admin_check:bool = false;

        match &self.players[player_index as usize] {
            Some(player) => {
                let update = HQMMessage::PlayerUpdate {
                    player_name: player.player_name.clone(),
                    object: None,
                    player_index,
                    in_server: false,
                };
                if let Some(object_index) = player.skater {
                    self.game.world.objects[object_index] = HQMGameObject::None;
                }

                if player.is_admin{
                    admin_check=true;
                }

                self.add_global_message(update, true);

                self.players[player_index as usize] = None;
            }
            None => {

            }
        }

        if admin_check{
            let mut admin_found=false;

            for p in self.players.iter_mut() {
                if let Some(player) = p {
                    if player.is_admin{
                        admin_found=true;
                    }
                }
            }

            if !admin_found{
                self.allow_join=true;
            }
        }
    }

    fn add_user_chat_message(&mut self, message: String, sender_index: usize) {
        if let Some(player) = & self.players[sender_index] {
            info!("{} ({}): {}", &player.player_name, sender_index, &message);
            let chat = HQMMessage::Chat {
                player_index: Some(sender_index),
                message,
            };
            self.add_global_message(chat, false);
        }

    }

    pub(crate) fn add_server_chat_message(&mut self, message: String) {
        let chat = HQMMessage::Chat {
            player_index: None,
            message,
        };
        self.add_global_message(chat, false);
    }

    fn add_directed_user_chat_message2(&mut self, message: String, receiver_index: usize, sender_index: Option<usize>) {
        // This message will only be visible to a single player
        if let Some(player) = & mut self.players[receiver_index] {
            player.add_directed_user_chat_message2(message, sender_index);
        }
    }

    pub(crate) fn add_directed_user_chat_message(&mut self, message: String, receiver_index: usize, sender_index: usize) {
        self.add_directed_user_chat_message2(message, receiver_index, Some (sender_index));
    }

    pub(crate) fn add_directed_server_chat_message(&mut self, message: String, receiver_index: usize) {
        self.add_directed_user_chat_message2(message, receiver_index, None);
    }

    pub(crate) fn add_global_message(&mut self, message: HQMMessage, persistent: bool) {
        let rc = Rc::new(message);
        self.game.replay_messages.push(rc.clone());
        if persistent {
            self.game.persistent_messages.push(rc.clone());
        }
        for player in self.players.iter_mut() {
            match player {
                Some(player) => {
                    player.messages.push(rc.clone());
                }
                _ => ()
            }
        }
    }

    fn find_player_slot(&self, addr: SocketAddr) -> Option<usize> {
        return self.players.iter().position(|x| {
            match x {
                Some(x) => x.addr == addr,
                None => false
            }
        });
    }

    fn find_empty_player_slot(&self) -> Option<usize> {
        return self.players.iter().position(|x| x.is_none());
    }

    fn update_players_and_input(&mut self) {
        let mut red_player_count = 0usize;
        let mut blue_player_count = 0usize;
        for p in self.game.world.objects.iter () {
            if let HQMGameObject::Player(player) = p {
                if player.team == HQMTeam::Red {
                    red_player_count += 1;
                } else if player.team == HQMTeam::Blue {
                    blue_player_count += 1;
                }
            }
        }

        let mut messages = vec![];
        let mut chat_messages = vec![];
        let players = & mut self.players;
        let world = & mut self.game.world;
        for (player_index, player_option) in players.iter_mut().enumerate() {

            if let Some(player) = player_option {
                player.inactivity += 1;
                if player.inactivity > 500 {
                    if let Some(object_index) = player.skater {
                       world.objects[object_index] = HQMGameObject::None;
                    }
                    info!("{} ({}) timed out", player.player_name, player_index);
                    messages.push(HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone(),
                        object: None,
                        player_index,
                        in_server: false
                    });
                    let chat_msg = format!("{} timed out", player.player_name);
                    chat_messages.push(chat_msg);

                    *player_option = None;

                    continue;
                }

                player.team_switch_timer = player.team_switch_timer.saturating_sub(1);
                let skater_object = player.skater.and_then(|x| match & mut world.objects[x] {

                    HQMGameObject::Player(player) => { Some(player)}
                    _ => None,
                });
                let change = match skater_object {
                    Some(skater_object) => {

                        if player.input.spectate() {
                            let team_player_count = match skater_object.team {
                                HQMTeam::Red => & mut red_player_count,
                                HQMTeam::Blue => & mut blue_player_count
                            };
                            let res = set_team_internal(player_index, player, world, & self.config, None);
                            if res.is_some() {
                                *team_player_count -= 1;
                                player.team_switch_timer = 500;
                            }
                            res

                        } else {
                            skater_object.input = player.input.clone();
                            None
                        }
                    }
                    None => {
                        let new_team =
                            if player.input.join_red() {
                                Some ((HQMTeam::Red, & mut red_player_count, blue_player_count))
                            } else if player.input.join_blue() {
                                Some ((HQMTeam::Blue, & mut blue_player_count, red_player_count))
                            } else {
                                None
                            };
                        if let Some((new_team, new_team_count, other_team_count)) = new_team {
                            if player.team_switch_timer == 0 && *new_team_count + 1 <= self.config.team_max && (!self.config.force_team_size_parity || (*new_team_count <= other_team_count)) {
                                let res = set_team_internal(player_index, player, world, & self.config, Some(new_team));
                                if res.is_some() {
                                    *new_team_count += 1;
                                }
                                res
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                };
                if let Some(change) = change  {
                    messages.push(HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone(),
                        object: change,
                        player_index,
                        in_server: true
                    });
                }

            }
        }

        for message in messages {
            self.add_global_message(message, true);
        }
        for message in chat_messages {
            self.add_server_chat_message(message);
        }
    }


    async fn tick(&mut self, socket: & UdpSocket) {
        if self.player_count() != 0 {
            self.game.active = true;
            let packets = tokio::task::block_in_place(|| {
                self.update_players_and_input();
                let events = self.game.world.simulate_step();
                if self.config.mode == HQMServerMode::Match {
                    self.handle_events(events);
                    self.update_clock();
                    self.game.update_game_state();
                }

                get_packets(& self.game.world.objects)
            });

            let mut write_buf = vec![0u8; 4096];
            self.game.saved_ticks.truncate(self.game.saved_ticks.capacity() - 1);
            self.game.saved_ticks.push_front(HQMSavedTick {
                packets,
                time: Instant::now()
            });

            self.game.packet = self.game.packet.wrapping_add(1);
            self.game.game_step = self.game.game_step.wrapping_add(1);

            send_updates(&self.game, & self.players, socket, & mut write_buf).await;
            if self.config.replays_enabled {
                write_replay(& mut self.game, & mut write_buf);
            }
        } else if self.game.active {
            info!("Game {} abandoned", self.game.game_id);
            self.new_game();
            self.allow_join=true;
        }

    }

    fn call_goal (& mut self, team: HQMTeam, puck: usize) {
        let (new_score, opponent_score) = match team {
            HQMTeam::Red => {
                self.game.red_score += 1;
                (self.game.red_score, self.game.blue_score)
            }
            HQMTeam::Blue => {
                self.game.blue_score += 1;
                (self.game.blue_score, self.game.red_score)
            }
        };

        self.game.time_break = self.config.time_break *100;
        self.game.is_intermission_goal = true;
        self.game.next_faceoff_spot = self.game.world.rink.center_faceoff_spot.clone();

        let game_over = if self.game.period > 3 && self.game.red_score != self.game.blue_score {
            true
        } else if self.config.mercy > 0 && (new_score - opponent_score) >= self.config.mercy {
            true
        } else if self.config.first_to > 0 && new_score >= self.config.first_to {
            true
        } else {
            false
        };

        if game_over {
            self.game.time_break = self.config.time_intermission*100;
            self.game.game_over = true;
        }

        let mut goal_scorer_index = None;
        let mut assist_index = None;

        if let HQMGameObject::Puck(this_puck) = & mut self.game.world.objects[puck] {
            for touch in this_puck.touches.iter() {
                if touch.team == team {
                    let player_index = touch.player_index;
                    if goal_scorer_index.is_none() {
                        goal_scorer_index = Some(player_index);
                    } else if assist_index.is_none() && Some(player_index) != goal_scorer_index {
                        assist_index = Some(player_index);
                        break;
                    }
                }
            }
        }

        let message = HQMMessage::Goal {
            team,
            goal_player_index: goal_scorer_index,
            assist_player_index: assist_index
        };
        self.add_global_message(message, true);
    }

    fn call_offside(&mut self, team: HQMTeam, pass_origin: &Point3<f32>) {

        self.game.next_faceoff_spot = self.game.world.rink.get_offside_faceoff_spot(pass_origin, team);
        self.game.time_break = self.config.time_break *100;
        self.game.offside_status = HQMOffsideStatus::Offside(team);
        self.add_server_chat_message(String::from("Offside"));
    }

    fn call_icing(& mut self, team: HQMTeam, pass_origin: &Point3<f32>) {
        self.game.next_faceoff_spot = self.game.world.rink.get_icing_faceoff_spot(pass_origin, team);
        self.game.time_break = self.config.time_break *100;
        self.game.icing_status = HQMIcingStatus::Icing(team);
        self.add_server_chat_message(String::from("Icing"));
    }

    fn handle_events (& mut self, events: Vec<HQMSimulationEvent>) {
        if self.game.offside_status.is_offside()
            || self.game.icing_status.is_icing()
        || self.game.period == 0
        || self.game.time == 0
        || self.game.time_break > 0
        || self.game.paused {
            return;
        }
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet {
                    team, puck
                } => {
                    match &self.game.offside_status {
                        HQMOffsideStatus::Warning(offside_team, p, _) if *offside_team == team => {
                            let copy = p.clone();
                            self.call_offside(team, &copy);
                        }
                        HQMOffsideStatus::Offside(_) => {},
                        _ => {
                            self.call_goal(team, puck);
                        }
                    }
                },
                HQMSimulationEvent::PuckTouch {
                    player, puck
                } => {
                    // Get connected player index from skater
                    if let HQMGameObject::Player(skater) = & self.game.world.objects[player] {
                        let this_connected_player_index = skater.connected_player_index;
                        let touching_team = skater.team;
                        let faceoff_position = skater.faceoff_position.clone();

                        if let HQMGameObject::Puck(puck) = & mut self.game.world.objects[puck] {
                            puck.add_touch(this_connected_player_index, touching_team, self.game.time);

                            let other_team = match touching_team {
                                HQMTeam::Red => HQMTeam::Blue,
                                HQMTeam::Blue => HQMTeam::Red
                            };

                            if let HQMOffsideStatus::Warning(team, p, i) = &self.game.offside_status {
                                if *team == touching_team {
                                    let pass_origin = if this_connected_player_index == *i {
                                        puck.body.pos.clone()
                                    } else {
                                        p.clone()
                                    };
                                    self.call_offside(touching_team, &pass_origin);
                                }
                                continue;

                            }
                            if let HQMIcingStatus::Warning(team, p) = &self.game.icing_status {
                                if touching_team != *team {
                                    if faceoff_position == "G" {
                                        self.game.icing_status = HQMIcingStatus::No;
                                        self.add_server_chat_message(String::from("Icing waved off"));
                                    } else {
                                        let copy = p.clone();
                                        self.call_icing(other_team, &copy);
                                    }
                                } else {
                                    self.game.icing_status = HQMIcingStatus::No;
                                    self.add_server_chat_message(String::from("Icing waved off"));
                                }
                            } else if let HQMIcingStatus::NotTouched (_, _) = self.game.icing_status {
                                self.game.icing_status = HQMIcingStatus::No;
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOtherHalf {
                    team, puck
                } => {

                    if let HQMGameObject::Puck(puck) = & self.game.world.objects[puck] {
                        if let Some(touch) = puck.touches.front() {
                            if team == touch.team && self.game.icing_status == HQMIcingStatus::No {
                                self.game.icing_status = HQMIcingStatus::NotTouched(team, touch.puck_pos.clone());
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckPassedGoalLine {
                    team, puck: _
                } => {

                    if let HQMIcingStatus::NotTouched(icing_team, p) = &self.game.icing_status {
                        if team == *icing_team {
                            match self.config.icing {
                                HQMIcingConfiguration::Touch => {
                                    self.game.icing_status = HQMIcingStatus::Warning(team, p.clone());
                                    self.add_server_chat_message(String::from("Icing warning"));
                                }
                                HQMIcingConfiguration::NoTouch => {
                                    let copy = p.clone();
                                    self.call_icing(team, &copy);
                                }
                                HQMIcingConfiguration::Off => {}
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOffensiveZone {
                    team, puck
                } => {
                    if self.game.offside_status == HQMOffsideStatus::InNeutralZone {
                        if let HQMGameObject::Puck(puck) = & self.game.world.objects[puck] {
                            if let Some(touch) = puck.touches.front() {
                                if team == touch.team &&
                                    has_players_in_offensive_zone(& self.game.world, team) {
                                    match self.config.offside {
                                        HQMOffsideConfiguration::Delayed => {
                                            self.game.offside_status = HQMOffsideStatus::Warning(team, touch.puck_pos.clone(), touch.player_index);
                                            self.add_server_chat_message(String::from("Offside warning"));
                                        }
                                        HQMOffsideConfiguration::Immediate => {
                                            let copy = touch.puck_pos.clone();
                                            self.call_offside(team, &copy);
                                        },
                                        HQMOffsideConfiguration::Off => {
                                            self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                        }
                                    }
                                } else {
                                    self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                                }
                            } else {
                                self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                            }
                        }
                    }

                },
                HQMSimulationEvent::PuckLeftOffensiveZone {
                    team: _, puck: _
                } => {
                    if let HQMOffsideStatus::Warning(_, _, _) = self.game.offside_status {
                        self.add_server_chat_message(String::from("Offside waved off"));
                    }
                    self.game.offside_status = HQMOffsideStatus::InNeutralZone;

                }
            }
        }
        if let HQMOffsideStatus::Warning(team, _, _) = self.game.offside_status {
            if !has_players_in_offensive_zone(& self.game.world,team) {
                self.game.offside_status = HQMOffsideStatus::InOffensiveZone(team);
                self.add_server_chat_message(String::from("Offside waved off"));
            }
        }
    }


    pub(crate) fn new_game(&mut self) {

        let old_game = std::mem::replace(& mut self.game, HQMGame::new(self.game_alloc, &self.config));

        if self.config.replays_enabled && old_game.period != 0 {
            let time = old_game.start_time.format("%Y-%m-%dT%H%M%S").to_string();
            let file_name = format!("{}.{}.hrp", self.config.server_name, time);
            let replay_data = old_game.replay_data;

            let game_id = old_game.game_id;

            tokio::spawn(async move {
                if tokio::fs::create_dir_all("replays").await.is_err() {
                    return;
                };
                let path: PathBuf = ["replays", &file_name].iter().collect();

                let mut file_handle = match File::create(path).await {
                    Ok(file) => file,
                    Err(e) => {
                        println!("{:?}", e);
                        return;
                    }
                };

                let size = replay_data.len() as u32;

                let _x = file_handle.write_all(&0u32.to_le_bytes()).await;
                let _x = file_handle.write_all(&size.to_le_bytes()).await;
                let _x = file_handle.write_all(&replay_data).await;
                let _x = file_handle.sync_all().await;

                info!("Replay of game {} saved as {}", game_id, file_name);
            });
        }

        info!("New game {} started", self.game.game_id);
        self.game_alloc += 1;

        let puck_line_start= self.game.world.rink.width / 2.0 - 0.4 * ((self.config.warmup_pucks - 1) as f32);

        for i in 0..self.config.warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8*(i as f32), 1.5, self.game.world.rink.length / 2.0);
            let rot = Matrix3::identity();
            self.game.world.create_puck_object(pos, rot);
        }

        let mut messages = Vec::new();
        for (i, p) in self.players.iter_mut().enumerate() {
            if let Some(player) = p {
                player.skater = None;

                player.known_msgpos = 0;
                player.known_packet = u32::MAX;
                player.messages.clear();
                let update = HQMMessage::PlayerUpdate {
                    player_name: player.player_name.clone(),
                    object: None,
                    player_index: i,
                    in_server: true,
                };
                messages.push(update);
            }


        }
        for message in messages {
            self.add_global_message(message, true);
        }

        self.game.time = self.config.time_warmup * 100;

    }

    fn get_faceoff_positions (players: & [Option<HQMConnectedPlayer>], objects: & [HQMGameObject], allowed_positions: &[String]) -> HashMap<usize, (HQMTeam, String)> {
        let mut res = HashMap::new();

        let mut red_players= vec![];
        let mut blue_players = vec![];
        for (player_index, player) in players.iter().enumerate() {
            if let Some(player) = player {
                let team = player.skater.and_then(|i| match &objects[i] {
                    HQMGameObject::Player(skater) => { Some(skater.team)},
                    _ => None
                });
                if team == Some(HQMTeam::Red) {
                    red_players.push((player_index, player.preferred_faceoff_position.as_ref()));
                } else if team == Some(HQMTeam::Blue) {
                    blue_players.push((player_index, player.preferred_faceoff_position.as_ref()));
                }

            }
        }

        fn setup_position (positions: & mut HashMap<usize, (HQMTeam, String)>, players: &[(usize, Option<&String>)], allowed_positions: &[String], team: HQMTeam) {
            let mut available_positions = Vec::from(allowed_positions);

            // First, we try to give each player its preferred position
            for (player_index, player_position) in players.iter() {
                if let Some(player_position) = player_position {
                    if let Some(x) = available_positions.iter().position(|x| *x == **player_position) {
                        let s = available_positions.remove(x);
                        positions.insert(*player_index, (team, s));
                    }
                }
            }
            let c = String::from("C");
            // Some players did not get their preferred positions because they didn't have one,
            // or because it was already taken
            for (player_index, player_position) in players.iter() {
                if !positions.contains_key(player_index) {

                    let s = if let Some(x) = available_positions.iter().position(|x| *x == c) {
                        // Someone needs to be C
                        available_positions.remove(x);
                        (team, c.clone())
                    } else if !available_positions.is_empty() {
                        // Give out the remaining positions
                        let x = available_positions.remove(0);
                        (team, x)
                    } else {
                        // Oh no, we're out of legal starting positions
                        if let Some(player_position) = player_position {
                            (team, (*player_position).clone())
                        } else {
                            (team, c.clone())
                        }
                    };
                    positions.insert(*player_index, s);
                }
            }
            if available_positions.contains(&c) && !players.is_empty() {
                positions.insert(players[0].0, (team, c.clone()));
            }
        }

        setup_position(& mut res, &red_players, allowed_positions, HQMTeam::Red);
        setup_position(& mut res, &blue_players, allowed_positions, HQMTeam::Blue);

        res
    }

    fn do_faceoff(&mut self){
        let faceoff_spot = &self.game.next_faceoff_spot;

        let positions = Self::get_faceoff_positions(& self.players, & self.game.world.objects,
                                                    &self.game.world.rink.allowed_positions);

        let puck_pos = &faceoff_spot.center_position + &(1.5f32*Vector3::y());

        self.game.world.objects = vec![HQMGameObject::None; 32];
        self.game.world.create_puck_object(puck_pos.clone(), Matrix3::identity());

        let mut messages = Vec::new();

        fn setup (messages: & mut Vec<HQMMessage>, world: & mut HQMGameWorld,
                  player: & mut HQMConnectedPlayer, player_index: usize, faceoff_position: String, pos: Point3<f32>, rot: Matrix3<f32>, team: HQMTeam) {
            let new_object_index = world.create_player_object(team,pos, rot, player.hand, player_index, faceoff_position, player.mass);
            player.skater = new_object_index;

            let update = HQMMessage::PlayerUpdate {
                player_name: player.player_name.clone(),
                object: new_object_index.map(|x| (x, team)),
                player_index,

                in_server: true,
            };
            messages.push(update);
        }

        for (player_index, (team, faceoff_position)) in positions {
            if let Some(player) = & mut self.players[player_index] {
                let (player_position, player_rotation) = match team {
                    HQMTeam::Red => {
                        faceoff_spot.red_player_positions[&faceoff_position].clone()
                    }
                    HQMTeam::Blue => {
                        faceoff_spot.blue_player_positions[&faceoff_position].clone()
                    }
                };
                setup (& mut messages, & mut self.game.world, player, player_index, faceoff_position, player_position,
                       player_rotation.matrix().clone_owned(), team)
            }

        }

        let rink = &self.game.world.rink;
        self.game.icing_status = HQMIcingStatus::No;
        self.game.offside_status = if rink.red_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Red)
        } else if rink.blue_lines_and_net.offensive_line.point_past_middle_of_line(&puck_pos) {
            HQMOffsideStatus::InOffensiveZone(HQMTeam::Blue)
        } else {
            HQMOffsideStatus::InNeutralZone
        };

        for message in messages {
            self.add_global_message(message, true);
        }

    }

    fn update_clock(&mut self) {
        if !self.game.paused {
            if self.game.period == 0 && self.game.time > 2000 {
                let mut has_red_players = false;
                let mut has_blue_players = false;
                for object in self.game.world.objects.iter() {
                    if let HQMGameObject::Player(skater) = object {
                        match skater.team {
                            HQMTeam::Red => {
                                has_red_players = true;
                            },
                            HQMTeam::Blue => {
                                has_blue_players = true;
                            },
                        }
                    }
                    if has_red_players && has_blue_players {
                        self.game.time = 2000;
                        break;
                    }
                }
            }

            if self.game.time_break > 0 {
                self.game.time_break -= 1;
                if self.game.time_break == 0 {
                    self.game.is_intermission_goal = false;
                    if self.game.game_over {
                        self.new_game();
                    } else {
                        if self.game.time == 0 {
                            self.game.time = self.config.time_period*100;
                        }
                        self.do_faceoff();
                    }

                }
            } else if self.game.time > 0 {
                self.game.time -= 1;
                if self.game.time == 0 {
                    self.game.period += 1;
                    if self.game.period > 3 && self.game.red_score != self.game.blue_score {
                        self.game.time_break = self.config.time_intermission*100;
                        self.game.game_over = true;
                    } else {
                        self.game.time_break = self.config.time_intermission*100;
                        self.game.next_faceoff_spot = self.game.world.rink.center_faceoff_spot.clone();
                    }
                }
            }

        }
    }

    pub async fn run(&mut self) -> std::io::Result<()> {

        // Start new game
        self.new_game();

        // Set up timers
        let mut tick_timer = tokio::time::interval(Duration::from_millis(10));

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));

        let socket = Arc::new (tokio::net::UdpSocket::bind(& addr).await?);
        info!("Server listening at address {:?}", socket.local_addr().unwrap());

        if self.config.public {
            let socket = socket.clone();
            tokio::spawn(async move {
                loop {
                    let master_server = get_master_server().await.ok();
                    if let Some (addr) = master_server {
                        for _ in 0..60 {
                            let msg = b"Hock\x20";
                            let res = socket.send_to(msg, addr).await;
                            if res.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    } else {
                        tokio::time::sleep(Duration::from_secs(15)).await;
                    }
                }
            });
        }
        let (msg_sender, mut msg_receiver) = tokio::sync::mpsc::channel(256);
        {
            let socket = socket.clone();

            tokio::spawn(async move {
                loop {
                    let mut buf = BytesMut::new();
                    buf.resize(512, 0u8);

                    match socket.recv_from(&mut buf).await {
                        Ok((size, addr)) => {
                            buf.truncate(size);
                            let _ = msg_sender.send(HQMServerReceivedData::GameClientPacket {
                                addr,
                                data: buf.freeze()
                            }).await;
                        }
                        Err(_) => {}
                    }
                }
            });
        };


        loop {
            tokio::select! {
                _ = tick_timer.tick() => {
                    self.tick(& socket).await;
                }
                x = msg_receiver.recv() => {
                    if let Some (HQMServerReceivedData::GameClientPacket {
                        addr,
                        data: msg
                    }) = x {
                        self.handle_message(addr, & socket, & msg).await;
                    }
                }
            }
        }
    }

    pub fn new(config: HQMServerConfiguration) -> Self {
        let mut player_vec = Vec::with_capacity(64);
        for _ in 0..64 {
            player_vec.push(None);
        }

        HQMServer {
            players: player_vec,
            ban_list: HashSet::new(),
            allow_join:true,
            game: HQMGame::new(1, &config),
            game_alloc: 1,
            is_muted:false,
            config,
        }
    }
}

fn has_players_in_offensive_zone (world: & HQMGameWorld, team: HQMTeam) -> bool {
    let line = match team {
        HQMTeam::Red => & world.rink.red_lines_and_net.offensive_line,
        HQMTeam::Blue => & world.rink.blue_lines_and_net.offensive_line,
    };

    for object in world.objects.iter() {
        if let HQMGameObject::Player(skater) = object {
            if skater.team == team {
                let feet_pos = &skater.body.pos - (&skater.body.rot * Vector3::y().scale(skater.height));
                let dot = (&feet_pos - &line.point).dot (&line.normal);
                let leading_edge = -(line.width/2.0);
                if dot < leading_edge {
                    // Player is offside
                    return true;
                }
            }
        }
    }

    false
}

fn write_message(writer: & mut HQMMessageWriter, message: &HQMMessage) {
    match message {
        HQMMessage::Chat {
            player_index,
            message
        } => {
            writer.write_bits(6, 2);
            writer.write_bits(6, match *player_index {
                Some(x)=> x as u32,
                None => u32::MAX
            });
            let message_bytes = message.as_bytes();
            let size = min(63, message_bytes.len());
            writer.write_bits(6, size as u32);

            for i in 0..size {
                writer.write_bits(7, message_bytes[i] as u32);
            }
        }
        HQMMessage::Goal {
            team,
            goal_player_index,
            assist_player_index
        } => {
            writer.write_bits(6, 1);
            writer.write_bits(2, team.get_num());
            writer.write_bits(6, match *goal_player_index {
                Some (x) => x as u32,
                None => u32::MAX
            });
            writer.write_bits(6, match *assist_player_index {
                Some (x) => x as u32,
                None => u32::MAX
            });
        }
        HQMMessage::PlayerUpdate {
            player_name,
            object,
            player_index,
            in_server,
        } => {
            writer.write_bits(6, 0);
            writer.write_bits(6, *player_index as u32);
            writer.write_bits(1, if *in_server { 1 } else { 0 });
            let (object_index, team_num) = match object {
                Some((i, team)) => {
                    (*i as u32, team.get_num ())
                },
                None => {
                    (u32::MAX, u32::MAX)
                }
            };
            writer.write_bits(2, team_num);
            writer.write_bits(6, object_index);

            let name_bytes = player_name.as_bytes();
            for i in 0usize..31 {
                let v = if i < name_bytes.len() {
                    name_bytes[i]
                } else {
                    0
                };
                writer.write_bits(7, v as u32);
            }
        }
    };
}



fn write_objects(writer: & mut HQMMessageWriter, game: &HQMGame, packets: &VecDeque<HQMSavedTick>, known_packet: u32) {
    let current_packets = &packets[0].packets;

    let old_packets = {
        let diff = if known_packet == u32::MAX {
            None
        } else {
            game.packet.checked_sub(known_packet)
        };
        if let Some(diff) = diff {
            let index = diff as usize;
            if index < packets.len() && index < 192 && index > 0 {
                Some(&packets[index].packets)
            } else {
                None
            }
        } else {
            None
        }
    };

    writer.write_u32_aligned(game.packet);
    writer.write_u32_aligned(known_packet);

    for i in 0..32 {
        let current_packet = &current_packets[i];
        let old_packet = old_packets.map(|x| &x[i]);
        match current_packet {
            HQMObjectPacket::Puck(puck) => {
                let old_puck = old_packet.and_then(|x| match x {
                    HQMObjectPacket::Puck(old_puck) => Some(old_puck),
                    _ => None
                });
                writer.write_bits(1, 1);
                writer.write_bits(2, 1); // Puck type
                writer.write_pos(17, puck.pos.0, old_puck.map(|puck| puck.pos.0));
                writer.write_pos(17, puck.pos.1, old_puck.map(|puck| puck.pos.1));
                writer.write_pos(17, puck.pos.2, old_puck.map(|puck| puck.pos.2));
                writer.write_pos(31, puck.rot.0, old_puck.map(|puck| puck.rot.0));
                writer.write_pos(31, puck.rot.1, old_puck.map(|puck| puck.rot.1));
            } ,
            HQMObjectPacket::Skater(skater) => {
                let old_skater = old_packet.and_then(|x| match x {
                    HQMObjectPacket::Skater(old_skater) => Some(old_skater),
                    _ => None
                });
                writer.write_bits(1, 1);
                writer.write_bits(2, 0); // Skater type
                writer.write_pos(17, skater.pos.0, old_skater.map(|skater| skater.pos.0));
                writer.write_pos(17, skater.pos.1, old_skater.map(|skater| skater.pos.1));
                writer.write_pos(17, skater.pos.2, old_skater.map(|skater| skater.pos.2));
                writer.write_pos(31, skater.rot.0, old_skater.map(|skater| skater.rot.0));
                writer.write_pos(31, skater.rot.1, old_skater.map(|skater| skater.rot.1));
                writer.write_pos(13, skater.stick_pos.0, old_skater.map(|skater| skater.stick_pos.0));
                writer.write_pos(13, skater.stick_pos.1, old_skater.map(|skater| skater.stick_pos.1));
                writer.write_pos(13, skater.stick_pos.2, old_skater.map(|skater| skater.stick_pos.2));
                writer.write_pos(25, skater.stick_rot.0, old_skater.map(|skater| skater.stick_rot.0));
                writer.write_pos(25, skater.stick_rot.1, old_skater.map(|skater| skater.stick_rot.1));
                writer.write_pos(16, skater.head_rot, old_skater.map(|skater| skater.head_rot));
                writer.write_pos(16, skater.body_rot, old_skater.map(|skater| skater.body_rot));
            },
            HQMObjectPacket::None => {
                writer.write_bits(1, 0);
            }
        }
    }
}

fn write_replay (game: & mut HQMGame, write_buf: & mut [u8]) {

    let mut writer = HQMMessageWriter::new(write_buf);

    writer.write_byte_aligned(5);
    writer.write_bits(1, match game.game_over {
        true => 1,
        false => 0
    });
    writer.write_bits(8, game.red_score);
    writer.write_bits(8, game.blue_score);
    writer.write_bits(16, game.time);

    writer.write_bits(16,
                      if game.is_intermission_goal {
                          game.time_break
                      }
                      else {0});
    writer.write_bits(8, game.period);

    let packets = &game.saved_ticks;

    write_objects(& mut writer, game, packets, game.replay_last_packet);
    game.replay_last_packet = game.packet;

    let remaining_messages = game.replay_messages.len() - game.replay_msg_pos;

    writer.write_bits(16, remaining_messages as u32);
    writer.write_bits(16, game.replay_msg_pos as u32);

    for message in &game.replay_messages[game.replay_msg_pos..game.replay_messages.len()] {
        write_message(& mut writer, Rc::as_ref(message));
    }
    game.replay_msg_pos = game.replay_messages.len();

    let pos = writer.get_pos();

    let slice = &write_buf[0..pos+1];

    game.replay_data.extend_from_slice(slice);
}

async fn send_updates(game: &HQMGame, players: &[Option<HQMConnectedPlayer>], socket: & UdpSocket, write_buf: & mut [u8]) {

    let packets = &game.saved_ticks;

    let rules_state =
        if let HQMOffsideStatus::Offside(_) = game.offside_status{
            HQMRulesState::Offside
        } else if let HQMIcingStatus::Icing(_) = game.icing_status {
            HQMRulesState::Icing
        } else {
            let icing_warning = game.icing_status.is_warning();
            let offside_warning = game.offside_status.is_warning();
            HQMRulesState::Regular {
                offside_warning, icing_warning
            }
        };

    for player in players.iter() {
        if let Some(player) = player {
            let mut writer = HQMMessageWriter::new(write_buf);

            if player.game_id != game.game_id {
                writer.write_bytes_aligned(GAME_HEADER);
                writer.write_byte_aligned(6);
                writer.write_u32_aligned(game.game_id);
            } else {
                writer.write_bytes_aligned(GAME_HEADER);
                writer.write_byte_aligned(5);
                writer.write_u32_aligned(game.game_id);
                writer.write_u32_aligned(game.game_step);
                writer.write_bits(1, match game.game_over {
                    true => 1,
                    false => 0
                });
                writer.write_bits(8, game.red_score);
                writer.write_bits(8, game.blue_score);
                writer.write_bits(16, game.time);

                writer.write_bits(16,
                                  if game.is_intermission_goal {
                                      game.time_break
                                  }
                                  else {0});
                writer.write_bits(8, game.period);
                writer.write_bits(8, player.view_player_index as u32);

                // if using a non-cryptic version, send ping
                if player.client_version > 0 {
                    writer.write_u32_aligned(player.deltatime);
                }

                // if baba's second version or above, send rules
                if player.client_version > 1 {
                    let num = match rules_state {
                        HQMRulesState::Regular { offside_warning, icing_warning } => {
                            let mut res = 0;
                            if offside_warning {
                                res |= 1;
                            }
                            if icing_warning {
                                res |= 2;
                            }
                            res
                        }
                        HQMRulesState::Offside => {
                            4
                        }
                        HQMRulesState::Icing => {
                            8
                        }
                    };
                    writer.write_u32_aligned(num);
                }

                write_objects(& mut writer, game, packets, player.known_packet);

                let remaining_messages = min(player.messages.len() - player.known_msgpos, 15);

                writer.write_bits(4, remaining_messages as u32);
                writer.write_bits(16, player.known_msgpos as u32);

                for message in &player.messages[player.known_msgpos..player.known_msgpos + remaining_messages] {
                    write_message(& mut writer, Rc::as_ref(message));
                }
            }
            let bytes_written = writer.get_bytes_written();

            let slice = &write_buf[0..bytes_written];
            let _ = socket.send_to(slice, player.addr).await;
        }
    }

}

fn set_team_internal (player_index: usize, player: & mut HQMConnectedPlayer, world: & mut HQMGameWorld, config: & HQMServerConfiguration, team: Option<HQMTeam>)
                      -> Option<Option<(usize, HQMTeam)>> {
    let current_skater = player.skater.and_then(|skater_index| match & mut world.objects[skater_index] {
        HQMGameObject::Player(skater) => {
            Some((skater_index, skater))
        }
        _ => None
    });
    match current_skater {
        Some((skater_index, current_skater)) => {
            match team {
                Some(team) => {
                    if current_skater.team != team {
                        current_skater.team = team;
                        info!("{} ({}) has switched to team {:?}", player.player_name, player_index, team);
                        Some(Some((skater_index, team)))
                    } else {
                        None
                    }
                },
                None => {
                    player.team_switch_timer = 500; // 500 ticks, 5 seconds
                    info!("{} ({}) is spectating", player.player_name, player_index);
                    world.objects[skater_index] = HQMGameObject::None;
                    player.skater = None;
                    Some(None)
                }
            }
        },
        None => {
            match team {
                Some(team) => {

                    let (pos, rot) = match config.spawn_point {
                        HQMSpawnPoint::Center => {
                            let (z, rot) = match team {
                                HQMTeam::Red => ((world.rink.length/2.0) + 3.0, 0.0),
                                HQMTeam::Blue => ((world.rink.length/2.0) - 3.0, PI)
                            };
                            let pos = Point3::new (world.rink.width / 2.0, 2.0, z);
                            let rot = Rotation3::from_euler_angles(0.0,rot,0.0);
                            (pos, rot)

                        }
                        HQMSpawnPoint::Bench => {
                            let z = match team {
                                HQMTeam::Red => (world.rink.length/2.0) + 4.0,
                                HQMTeam::Blue => (world.rink.length/2.0) - 4.0
                            };
                            let pos = Point3::new (0.5, 2.0, z);
                            let rot = Rotation3::from_euler_angles(0.0,3.0 * FRAC_PI_2,0.0);
                            (pos, rot)
                        }
                    };
                    if let Some(i) = world.create_player_object(team, pos, rot.matrix().clone_owned(), player.hand, player_index, "".to_string(), player.mass) {
                        player.skater = Some(i);
                        player.view_player_index = player_index;
                        info!("{} ({}) has joined team {:?}", player.player_name, player_index, team);
                        Some(Some((i, team)))
                    } else {
                        None
                    }
                },
                None => {
                    None
                }
            }
        }
    }
}

fn get_packets (objects: &[HQMGameObject]) -> Vec<HQMObjectPacket> {
    let mut packets: Vec<HQMObjectPacket> = Vec::with_capacity(32);
    for i in 0usize..32 {
        let packet = match &objects[i] {
            HQMGameObject::Puck(puck) => HQMObjectPacket::Puck(puck.get_packet()),
            HQMGameObject::Player(player) => HQMObjectPacket::Skater(player.get_packet()),
            HQMGameObject::None => HQMObjectPacket::None
        };
        packets.push(packet);
    }
    packets
}

fn get_player_name(bytes: Vec<u8>) -> Option<String> {
    let first_null = bytes.iter().position(|x| *x == 0);

    let bytes = match first_null {
        Some(x) => &bytes[0..x],
        None => &bytes[..]
    }.to_vec();
    return match String::from_utf8(bytes) {
        Ok(s) => {
            let s = s.trim();
            let s = if s.is_empty() {
                "Noname"
            } else {
                s
            };
            Some(String::from(s))
        } ,
        Err(_) => None
    };
}

async fn get_master_server () -> Result<SocketAddr, Box<dyn Error>> {
    let s = reqwest::get("http://www.crypticsea.com/anewzero/serverinfo.php")
        .await?.text().await?;

    let split = s.split_ascii_whitespace().collect::<Vec<&str>>();

    let addr = split.get(1).unwrap_or(&"").parse::<IpAddr> ()?;
    let port = split.get(2).unwrap_or(&"").parse::<u16> ()?;
    Ok(SocketAddr::new(addr, port))
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub(crate) enum HQMMuteStatus {
    NotMuted,
    ShadowMuted,
    Muted
}

pub(crate) struct HQMConnectedPlayer {
    pub(crate) player_name: String,
    pub(crate) addr: SocketAddr,
    client_version: u8,
    pub(crate) preferred_faceoff_position: Option<String>,
    pub(crate) skater: Option<usize>,
    game_id: u32,
    input: HQMPlayerInput,
    known_packet: u32,
    known_msgpos: usize,
    chat_rep: Option<u8>,
    messages: Vec<Rc<HQMMessage>>,
    inactivity: u32,
    pub(crate) is_admin: bool,
    pub(crate) is_muted: HQMMuteStatus,
    pub(crate) team_switch_timer: u32,
    hand: HQMSkaterHand,
    pub(crate) mass: f32,
    deltatime: u32,
    last_ping: VecDeque<f32>,
    view_player_index: usize
}

impl HQMConnectedPlayer {
    pub fn new(player_index: usize, player_name: String, addr: SocketAddr, global_messages: Vec<Rc<HQMMessage>>) -> Self {
        HQMConnectedPlayer {
            player_name,
            addr,
            client_version: 0,
            preferred_faceoff_position: None,
            skater: None,
            game_id: u32::MAX,
            known_packet: u32::MAX,
            known_msgpos: 0,
            chat_rep: None,
            messages: global_messages,
            input: HQMPlayerInput::default(),
            inactivity: 0,
            is_admin: false,
            is_muted: HQMMuteStatus::NotMuted,
            hand: HQMSkaterHand::Right,
            team_switch_timer: 0,
            // store latest deltime client sends you to respond with it
            deltatime: 0,
            last_ping: VecDeque::new (),
            view_player_index: player_index,
            mass: 1.0
        }
    }

    fn add_directed_user_chat_message2(&mut self, message: String, sender_index: Option<usize>) {
        // This message will only be visible to a single player
        let chat = HQMMessage::Chat {
            player_index: sender_index,
            message,
        };
        self.messages.push(Rc::new (chat));
    }

    #[allow(dead_code)]
    pub(crate) fn add_directed_user_chat_message(&mut self, message: String, sender_index: usize) {
        self.add_directed_user_chat_message2(message, Some (sender_index));
    }

    #[allow(dead_code)]
    pub(crate) fn add_directed_server_chat_message(&mut self, message: String) {
        self.add_directed_user_chat_message2(message, None);
    }

}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMIcingConfiguration {
    Off,
    Touch,
    NoTouch
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMOffsideConfiguration {
    Off,
    Delayed,
    Immediate
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMSpawnPoint {
    Center,
    Bench
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMServerMode {
    Match,
    PermanentWarmup
}

pub(crate) struct HQMServerConfiguration {
    pub(crate) server_name: String,
    pub(crate) port: u16,
    pub(crate) public: bool,
    pub(crate) player_max: usize,
    pub(crate) team_max: usize,
    pub(crate) force_team_size_parity: bool,
    pub(crate) welcome: Vec<String>,
    pub(crate) mode: HQMServerMode,

    pub(crate) password: String,

    pub(crate) time_period: u32,
    pub(crate) time_warmup: u32,
    pub(crate) time_break: u32,
    pub(crate) time_intermission: u32,
    pub(crate) mercy: u32,
    pub(crate) first_to: u32,
    pub(crate) offside: HQMOffsideConfiguration,
    pub(crate) icing: HQMIcingConfiguration,
    pub(crate) warmup_pucks: usize,

    pub(crate) cheats_enabled: bool,

    pub(crate) replays_enabled: bool,

    pub(crate) spawn_point: HQMSpawnPoint,

    pub(crate) physics_configuration: HQMPhysicsConfig,

}

