use std::net::{SocketAddr};

use nalgebra::{Vector3, Point3, Matrix3, Vector2, Rotation3};

use std::cmp::min;
use std::time::Duration;

use crate::hqm_parse::{HQMClientParser, HQMServerWriter, HQMObjectPacket};
use crate::hqm_simulate::HQMSimulationEvent;
use crate::hqm_game::{HQMTeam, HQMGameObject, HQMGameState, HQMSkaterHand, HQMGameWorld, HQMMessage, HQMGame, HQMPlayerInput, HQMFaceoffPosition};
use tokio::net::UdpSocket;
use std::rc::Rc;
use std::collections::HashSet;
use std::sync::Arc;


const GAME_HEADER: &[u8] = b"Hock";

const MASTER_SERVER: &str = "66.226.72.227:27590";

pub(crate) struct HQMServer {
    players: Vec<Option<HQMConnectedPlayer>>,
    ban_list: HashSet<std::net::IpAddr>,
    allow_join: bool,
    config: HQMServerConfiguration,
    game: HQMGame,
    game_alloc: u32,
    is_muted:bool,
}

impl HQMServer {
    async fn handle_message(&mut self, addr: SocketAddr, socket: & UdpSocket, msg: &[u8], write_buf: & mut [u8]) {
        let mut parser = HQMClientParser::new(&msg);
        let header = parser.read_bytes_aligned(4);
        if header != GAME_HEADER {
            return;
        }

        let command = parser.read_byte_aligned();
        match command {
            0 => {
                let _ = self.request_info(socket, &addr, &mut parser, write_buf).await;
            },
            2 => {
                self.player_join(&addr, &mut parser);
            },
            // if 8 or 0x10, client is modded, probly want to send it to the player_update function to store it in the client/player struct, to use when responding to clients
            4 | 8 | 0x10 => {
                self.player_update(&addr, &mut parser, command);
            },
            7 => {
                self.player_exit(&addr);
            },
            _ => {}
        }
    }

    async fn request_info<'a>(&self, socket: & UdpSocket, addr: &SocketAddr, parser: &mut HQMClientParser<'a>, write_buf: & mut [u8]) -> std::io::Result<usize> {
        let _player_version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();

        let mut writer = HQMServerWriter::new(write_buf);
        writer.write_bytes_aligned(GAME_HEADER);
        writer.write_byte_aligned(1);
        writer.write_bits(8, 55);
        writer.write_u32_aligned(ping);

        let player_count  = self.player_count();
        writer.write_bits(8, player_count);
        writer.write_bits(4, 4);
        writer.write_bits(4, self.config.team_max);

        writer.write_bytes_aligned_padded(32, self.config.server_name.as_ref());

        let slice = writer.get_slice();
        socket.send_to(slice, addr).await
    }

    fn player_count (& self) -> u32 {
        let mut player_count = 0u32;
        for player in &self.players {
            if player.is_some() {
                player_count += 1;
            }
        }
        player_count
    }

    fn player_update(&mut self, addr: &SocketAddr, parser: &mut HQMClientParser, command: u8) {
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
        if packet < player.packet && player.packet - packet < 1000 {
            // UDP does not guarantee that the packets arrive in the same order they were sent,
            // or at all. This should prevent packets that are older than the most recent one
            // received from being applied.
            return;
        }

        player.inactivity = 0;
        player.packet = packet;
        player.input = input;
        player.game_id = current_game_id;
        player.msgpos = parser.read_u16_aligned() as u32;


        let has_chat_msg = parser.read_bits(1) == 1;
        if has_chat_msg {
            let chat_rep = parser.read_bits(3);
            if chat_rep != player.chat_rep {
                player.chat_rep = chat_rep;
                let byte_num = parser.read_bits(8) as usize;
                let message = parser.read_bytes_aligned(byte_num);
                self.process_message(message, player_index);
            }
        }
    }

    fn admin_deny_message (& mut self, player_index: usize) {
        let msg = format!("Please log in before using that command");
        self.add_directed_server_chat_message(msg,player_index);

    }

    fn set_allow_join (& mut self, player_index: usize, allowed: bool) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.allow_join=allowed;

                if allowed {
                    let msg = format!("Joins enabled by {}",player.player_name);
                    self.add_server_chat_message(msg);
                } else {
                    let msg = format!("Joins disabled by {}",player.player_name);
                    self.add_server_chat_message(msg);
                }

            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn mute_player (& mut self, player_index: usize, mute_player: String) {
        if let Some(player) = & self.players[player_index]{
            if player.is_admin {
                let admin_player_name = player.player_name.clone();
                let mut player_found:bool = false;

                for p in self.players.iter_mut() {
                    if let Some(player) = p {
                        if player.player_name == mute_player{
                            player.is_muted=true;
                            player_found=true;
                        }
                    }
                }

                if player_found{
                    let msg = format!("{} muted by {}",mute_player,admin_player_name);
                    self.add_server_chat_message(msg);
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }

    }

    fn unmute_player (& mut self, player_index: usize, mute_player: String) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin {
                let admin_player_name = player.player_name.clone();
                let mut player_found:bool = false;

                for p in self.players.iter_mut() {
                    if let Some(player) = p {
                        if player.player_name == mute_player{
                            player.is_muted=false;
                            player_found=true;
                        }
                    }
                }

                if player_found{
                    let msg = format!("{} unmuted by {}",mute_player,admin_player_name);
                    self.add_server_chat_message(msg);
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn mute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=true;

                let msg = format!("Chat muted by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn unmute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=false;

                let msg = format!("Chat unmuted by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn force_player_off_ice (& mut self, player_index: usize, force_player_off_number: u32) {

        let mut admin_player_name = "".to_string();

        if let Some(player) = & self.players[player_index] {
            if player.is_admin {
                admin_player_name = player.player_name.clone();
            } else {
                self.admin_deny_message(player_index);
                return;
            }
        }

        let force_player_index_number = force_player_off_number - 1;

        if force_player_index_number as usize <= self.players.len()-1{
            if let Some(force_player) = & mut self.players[force_player_index_number as usize] {
                force_player.team = HQMTeam::Spec;
                force_player.team_switch_timer = 500; // 500 ticks, 5 seconds
                if let Some (i) = force_player.skater {
                    self.game.world.objects[i] = HQMGameObject::None;
                    force_player.skater = None;
                }
                let force_player_name = force_player.player_name.clone();
                let force_player_name_bytes = force_player.player_name.clone().into_bytes();
                self.add_global_message(HQMMessage::PlayerUpdate {
                    player_name: force_player_name_bytes,
                    team: HQMTeam::Spec,
                    player_index: force_player_index_number as usize,
                    object_index: None,
                    in_server: true
                },true);

                let msg = format!("{} forced off ice by {}",force_player_name,admin_player_name);
                self.add_server_chat_message(msg);
            }
        }


    }

    fn set_role (& mut self, player_index: usize, input_position:&str) {
        let found_position = self.config.faceoff_positions.iter().enumerate().find(|(_, position) | {
            position.abbreviation.to_lowercase() == input_position.to_lowercase()
        } );
        if let Some((role_index, position)) = found_position {
            if let Some(player) = & mut self.players[player_index] {
                player.faceoff_position_index = role_index;

                let msg = format!("{} position {}", player.player_name, position.abbreviation.to_uppercase());
                self.add_server_chat_message(msg);

            }
        }

    }

    fn admin_login (& mut self, player_index: usize, password:&str) {
        if let Some(player) = & mut self.players[player_index] {

            if self.config.password == password{
                player.is_admin = true;

                let msg = format!("{} admin", player.player_name);
                self.add_server_chat_message(msg);
            } else {

                let msg = format!("Incorrect password");
                self.add_directed_server_chat_message(msg,player_index);

            }
        }
    }

    fn kick_player (& mut self, admin_player_index: usize, kick_player_name: String, ban_player: bool) {

        if let Some(player) = & self.players[admin_player_index]{
            if player.is_admin {
                let admin_player_name = player.player_name.clone();

                // 0 full string | 1 begins with | 2 ends with | 3 contains
                let match_mode = if kick_player_name.starts_with("%"){
                    if kick_player_name.ends_with("%"){
                        3// %contains%
                    }else{
                        2// %ends with
                    }
                }else if kick_player_name.ends_with("%"){
                    1// begins with%
                } else {
                    0
                };

                // Because we allow matching using wildcards, we use vectors for multiple instances found
                let mut kick_player_list: Vec<(usize, String, SocketAddr)> = Vec::new();

                for (player_index, p) in self.players.iter_mut().enumerate() {
                    if let Some(player) = p {

                        match match_mode {
                            0 => { // full string
                                if player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            1 => { // begins with%
                                let match_string: String = kick_player_name.chars().take(kick_player_name.len()-1).collect();

                                if player.player_name.starts_with(&match_string) || player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            2 => { // %ends with
                                let match_string: String = kick_player_name.chars().skip(1).take(kick_player_name.len()-1).collect();

                                if player.player_name.ends_with(&match_string) || player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            3 => { // %contains%
                                let match_string: String = kick_player_name.chars().skip(1).take(kick_player_name.len()-2).collect();

                                if player.player_name.contains(&match_string) || player.player_name == kick_player_name{
                                    kick_player_list.push((player_index, player.player_name.clone(), player.addr));
                                }
                            },
                            _=>{}
                        }
                    }
                }
                if !kick_player_list.is_empty() {
                    for (player_index, player_name, player_addr) in kick_player_list {
                        if player_index != admin_player_index {
                            self.remove_player(player_index);

                            if ban_player{
                                self.ban_list.insert(player_addr.ip());

                                let msg = format!("{} banned by {}",player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            } else {
                                let msg = format!("{} kicked by {}",player_name, admin_player_name);
                                self.add_server_chat_message(msg);
                            }
                        } else {
                            if ban_player{
                                let msg = format!("You cannot ban yourself");
                                self.add_directed_server_chat_message(msg,admin_player_index);
                            } else {
                                let msg = format!("You cannot kick yourself");
                                self.add_directed_server_chat_message(msg,admin_player_index);
                            }
                        }
                    }

                } else {
                    match match_mode {
                        0 =>{ // full string
                            let msg = format!("No player names match {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        1 =>{ // begins with%
                            let msg = format!("No player names begin with {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        2 =>{ // %ends with
                            let msg = format!("No player names end with {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        3 =>{ // %contains%
                            let msg = format!("No player names contain {}",kick_player_name);
                            self.add_directed_server_chat_message(msg,admin_player_index);
                        },
                        _=>{}
                    }
                }

            } else{
                self.admin_deny_message(admin_player_index);
                return;
            }
        }

    }

    fn clear_bans (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.ban_list.clear();

                let msg = format!("Bans cleared by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn set_clock (& mut self, input_minutes: u32, input_seconds: u32, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.time = (input_minutes * 60 * 100)+ (input_seconds * 100);

                let msg = format!("Clock set by {}", player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }

    }

    fn set_score (& mut self, input_team: HQMTeam, input_score: u32,player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                match input_team {
                    HQMTeam::Red =>{
                        self.game.red_score = input_score;

                        let msg = format!("Red score changed by {}",player.player_name);
                        self.add_server_chat_message(msg);
                    },
                    HQMTeam::Blue =>{
                        self.game.blue_score = input_score;

                        let msg = format!("Blue score changed by {}",player.player_name);
                        self.add_server_chat_message(msg);
                    },
                    _=>{}
                }
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn set_period (& mut self, input_period: u32,player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{

                self.game.period = input_period;

                let msg = format!("Period set by {}",player.player_name);
                self.add_server_chat_message(msg);

            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn faceoff (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.timeout = 5*100;

                let msg = format!("Faceoff initiated by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn reset_game (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                let msg = format!("Game reset by {}",player.player_name);

                self.new_game();

                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn pause (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.paused=true;

                let msg = format!("Game paused by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn unpause (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.paused=false;

                let msg = format!("Game resumed by {}",player.player_name);
                self.add_server_chat_message(msg);
            } else {
                self.admin_deny_message(player_index);
            }
        }
    }

    fn set_hand (& mut self, hand: HQMSkaterHand, player_index: usize) {
        if let Some(player) = & mut self.players[player_index] {
            player.hand = hand;
            if let Some(skater_obj_index) = player.skater {
                if let HQMGameObject::Player(skater) = & mut self.game.world.objects[skater_obj_index] {

                    if self.game.state == HQMGameState::Game {
                        let msg = format!("Stick hand change will change after next intermission");
                        self.add_directed_server_chat_message(msg, player_index);

                        return;
                    }

                    skater.hand = hand;
                }
            }
        }

    }

    fn process_command (&mut self, command: &str, args: &[&str], player_index: usize) {

        match command{
            "enablejoin" => {
                self.set_allow_join(player_index,true);
            },
            "disablejoin" => {
                self.set_allow_join(player_index,false);
            },
            "muteplayer" => {
                if args.len() > 0{
                    self.mute_player(player_index,args.join(" "));
                }
            },
            "unmuteplayer" => {
                if args.len() > 0{
                    self.unmute_player(player_index,args.join(" "));
                }
            },
            "mutechat" => {
                self.mute_chat(player_index);
            },
            "unmute" => {
                self.unmute_chat(player_index);
            },
            "fs" => {
                if args.len() > 0{
                    self.force_player_off_ice(player_index,args.join(" ").parse::<u32>().unwrap());
                }
            },
            "kick" => {
                if args.len() > 0{
                    self.kick_player(player_index,args.join(" "),false);
                }
            },
            "ban" => {
                if args.len() > 0{
                    self.kick_player(player_index,args.join(" "),true);
                }
            },
            "clearbans" => {
                self.clear_bans(player_index);
            },
            "set" => {

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
                                _=>{

                                }
                            }

                        },
                        _ => {

                        }
                    }
                }

            },
            "sp" => {
                if args.len() == 1{
                    self.set_role(player_index,args[0]);
                }
            },
            "setposition" => {
                if args.len() == 1{
                    self.set_role(player_index,args[0]);
                }
            },
            "admin" => {
                if args.len() == 1{
                    self.admin_login(player_index,args[0]);
                }
            },
            "faceoff" => {
                self.faceoff(player_index);
            },
            "resetgame" => {
                self.reset_game(player_index);
            },
            "pause" => {
                self.pause(player_index);
            },
            "unpause" => {
                self.unpause(player_index);
            },
            "lefty" => {
                self.set_hand(HQMSkaterHand::Left, player_index);
            },
            "righty" => {
                self.set_hand(HQMSkaterHand::Right, player_index);
            },
            _ => {}, // matches have to be exhaustive
        }

        println! ("{} {:?}", command, args);
    }

    fn process_message(&mut self, bytes: Vec<u8>, player_index: usize) {
        let msg = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return
        };

        if self.players[player_index].is_some() {
            if msg.starts_with("/") {
                let split: Vec<&str> = msg.split(" ").collect(); // Temporary comment: this was changed from split_ascii_whitespace so that player names with spaces could be used as an argument for /kick etc (there appears to be no way to reconstruct such a name otherwise)
                let command = &split[0][1..];
                let args = &split[1..];
                self.process_command(command, args, player_index);
            } else {
                match &self.players[player_index as usize] {
                    Some(player) => {
                        if !player.is_muted && !self.is_muted {
                            self.add_user_chat_message(player_index, msg);
                        }
                    },
                    _=>{return;}
                }

            }
        }
    }

    fn player_exit(&mut self, addr: &SocketAddr) {
        let current_slot = self.find_player_slot(addr);
        match current_slot {
            Some(x) => {
                let player_name = {
                    let player = self.players[x].as_ref().unwrap();
                    player.player_name.clone()
                };
                self.remove_player(x);
                let msg = format!("{} exited", player_name);
                self.add_server_chat_message(msg);
            }
            None => {
                println!("Player has already exited");
            }
        }
    }

    fn add_player(&mut self, player_name: String, addr: &SocketAddr) -> bool {
        let player_index = self.find_empty_player_slot();
        match player_index {
            Some(x) => {
                let update = HQMMessage::PlayerUpdate {
                    player_name: player_name.clone().into_bytes(),
                    team: HQMTeam::Spec,
                    player_index: x,
                    object_index: None,
                    in_server: true,
                };

                self.add_global_message(update, true);

                let mut messages = self.game.global_messages.clone();
                for welcome_msg in self.config.welcome.iter() {
                    messages.push(Rc::new(HQMMessage::Chat {
                        player_index: None,
                        message: welcome_msg.clone().into_bytes()
                    }));
                }

                let new_player = HQMConnectedPlayer::new(player_name, *addr, messages);

                self.players[x] = Some(new_player);

                true
            }
            _ => false
        }
    }

    fn remove_player(&mut self, player_index: usize) {

        let mut admin_check:bool = false;

        match &self.players[player_index as usize] {
            Some(player) => {
                let update = HQMMessage::PlayerUpdate {
                    player_name: player.player_name.clone().into_bytes(),
                    team: HQMTeam::Spec,
                    player_index,
                    object_index: None,
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
                println!("Player has already exited");
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

    fn add_user_chat_message(&mut self, player_index: usize, message: String) {
        if let Some(player) = & self.players[player_index] {
            println!("{}: {}", &player.player_name, &message);
            let chat = HQMMessage::Chat {
                player_index: Some(player_index),
                message: message.into_bytes(),
            };
            self.add_global_message(chat, false);
        }

    }

    fn add_server_chat_message(&mut self, message: String) {
        println!("{}", &message);
        let chat = HQMMessage::Chat {
            player_index: None,
            message: message.into_bytes(),
        };
        self.add_global_message(chat, false);
    }

    fn add_directed_server_chat_message(&mut self, message: String, player_receiving_index: usize) {
        println!("{}", &message);
        // This message will only be visible to a single player
        let chat = HQMMessage::Chat {
            player_index: None,
            message: message.into_bytes(),
        };
        if let Some(player) = & mut self.players[player_receiving_index] {
            player.messages.push(Rc::new (chat));
        }
    }

    fn player_join(&mut self, addr: &SocketAddr, parser: &mut HQMClientParser) {
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
                if self.add_player(name.clone(), &addr) {
                    let msg = format!("{} joined", name);
                    self.add_server_chat_message(msg);
                }
            }
            _ => {}
        };
    }

    fn add_global_message(&mut self, message: HQMMessage, persistent: bool) {
        let rc = Rc::new(message);
        if persistent {
            self.game.global_messages.push(rc.clone());
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


    fn find_player_slot(&self, addr: &SocketAddr) -> Option<usize> {
        return self.players.iter().position(|x| {
            match x {
                Some(x) => x.addr == *addr,
                None => false
            }
        });
    }

    fn find_empty_player_slot(&self) -> Option<usize> {
        return self.players.iter().position(|x| x.is_none());
    }

    fn remove_inactive_players (& mut self) {
        for i in 0..self.players.len() {
            let inactivity = match & mut self.players[i] {
                Some(p) => {
                    p.inactivity += 1;
                    p.inactivity >= 500
                },
                None => false
            };
            if inactivity {
                let player_name = {
                    let player = self.players[i].as_ref().unwrap();
                    player.player_name.clone()
                };
                self.remove_player(i);
                let msg = format!("{} timed out", player_name);
                self.add_server_chat_message(msg);
            }
        }
    }


    fn move_players_between_teams(&mut self) {
        let mut red_player_count = 0;
        let mut blue_player_count = 0;
        for p in self.players.iter() {
            if let Some(player) = p {
                if player.team == HQMTeam::Red {
                    red_player_count += 1;
                } else if player.team == HQMTeam::Blue {
                    blue_player_count += 1;
                }
            }
        }
        let mut new_messages = Vec::new();
        for (player_index, p) in self.players.iter_mut().enumerate() {
            if let Some(player) = p {
                player.team_switch_timer = player.team_switch_timer.saturating_sub(1);
                if (player.input.join_red() || player.input.join_blue())
                    && player.team == HQMTeam::Spec
                    && player.team_switch_timer == 0 {
                    let (new_team, new_team_count, other_team_count) = if player.input.join_red() {
                        (HQMTeam::Red, & mut red_player_count, blue_player_count)
                    } else {
                        (HQMTeam::Blue, & mut blue_player_count, red_player_count)
                    };
                    if new_team != player.team && *new_team_count + 1 <= self.config.team_max
                        && (!self.config.force_team_size_parity || (*new_team_count <= other_team_count)) {
                        if player.skater.is_none() {

                            let mut pos = Point3::new(0.0,2.5,0.0);
                            let mut rot = Rotation3::from_euler_angles(0.0,0.0,0.0);

                            match new_team{
                                HQMTeam::Red=>{
                                    pos = Point3::new(self.config.entry_point_red[0],self.config.entry_point_red[1],self.config.entry_point_red[2]);
                                    rot = Rotation3::from_euler_angles(0.0,self.config.entry_rotation_red,0.0);

                                },
                                HQMTeam::Blue=>{
                                    pos = Point3::new(self.config.entry_point_blue[0],self.config.entry_point_blue[1],self.config.entry_point_blue[2]);
                                    rot = Rotation3::from_euler_angles(0.0,self.config.entry_rotation_blue,0.0);
                                },
                                _=>{}
                            }

                            if let Some(i) = self.game.world.create_player_object(pos, rot.matrix().clone_owned(), player.hand, player_index) {
                                player.team = new_team;
                                player.skater = Some(i);
                                *new_team_count += 1;
                            }
                        } else {
                            player.team = new_team;
                        }

                        // Message for new player
                        new_messages.push(HQMMessage::PlayerUpdate {
                            player_name: player.player_name.clone().into_bytes(),
                            team: player.team,
                            player_index,
                            object_index: player.skater,
                            in_server: true
                        });
                        println! ("{} {}", player_index, player.team);
                    }
                } else if player.input.spectate() && player.team != HQMTeam::Spec {
                    player.team = HQMTeam::Spec;
                    player.team_switch_timer = 500; // 500 ticks, 5 seconds
                    if let Some (i) = player.skater {
                        self.game.world.objects[i] = HQMGameObject::None;
                        player.skater = None;
                    }
                    new_messages.push(HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone().into_bytes(),
                        team: player.team,
                        player_index,
                        object_index: None,
                        in_server: true
                    });
                }
            }
        }
        for m in new_messages {
            self.add_global_message(m, true);
        }
    }

    fn copy_player_input_to_object(& mut self) {
        for p in self.players.iter() {
            if let Some (player) = p {
                if let Some (object_index) = player.skater {
                    if let HQMGameObject::Player(player_object) = & mut self.game.world.objects[object_index] {
                        player_object.input = player.input.clone();
                    }
                }
            }
        }
    }

    async fn tick(&mut self, socket: & UdpSocket, write_buf: & mut [u8]) {
        if self.player_count() != 0 {
            self.game.active = true;
            self.remove_inactive_players (); // connected players and objects
            self.move_players_between_teams();
            self.copy_player_input_to_object();
            let events = self.game.world.simulate_step();
            self.handle_events(events);
            self.update_clock();

            self.game.update_game_state();

            let packets = get_packets(& self.game.world.objects);

            for (i, x) in self.players.iter().enumerate() {
                if let Some(p) = x {
                    self.send_update(p, i, socket, &packets, write_buf).await;
                }
            }
            self.game.packet += 1;
            self.game.game_step += 1;
        } else if self.game.active {
            self.new_game();
            self.allow_join=true;
        }

    }

    fn handle_events (& mut self, events: Vec<HQMSimulationEvent>) {
        for event in events {
            match event {
                HQMSimulationEvent::EnteredNet {
                    team, net: _, puck
                } => {
                    if self.game.period > 0 &&
                        self.game.time > 0 &&
                        self.game.timeout == 0 {
                        let scoring_team = if team == HQMTeam::Red {
                            self.game.blue_score += 1;
                            HQMTeam::Blue
                        } else if team == HQMTeam::Blue {
                            self.game.red_score += 1;
                            HQMTeam::Red
                        } else {
                            panic!();
                        };
                        self.game.timeout = 700;
                        if self.game.period > 3 {
                            self.game.intermission = 2000;
                            self.game.game_over = true;
                        }

                        let mut goal_scorer_index = None;
                        let mut assist_index = None;

                        if let HQMGameObject::Puck(this_puck) = & mut self.game.world.objects[puck] {
                            let list = &this_puck.last_player_index;

                            for i in 0..4 {
                                if let Some(player_index) = list[i] {
                                    if let Some(player) = &self.players[player_index] {
                                        if player.team == scoring_team {

                                            if goal_scorer_index.is_none() {
                                                goal_scorer_index = Some(player_index);
                                            } else if assist_index.is_none() && Some(player_index) != goal_scorer_index {
                                                assist_index = Some(player_index);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }

                        }

                        let message = HQMMessage::Goal {
                            team: scoring_team,
                            goal_player_index: goal_scorer_index,
                            assist_player_index: assist_index
                        };
                        self.add_global_message(message, true);

                    }
                },
                HQMSimulationEvent::Touch {
                    player, puck
                } => {
                    // Get connected player index from skater
                    if let HQMGameObject::Player(this_skater) = & mut self.game.world.objects[player] {
                        let this_connected_player_index = this_skater.connected_player_index;

                        // Store player index in queue for awarding goals/assists
                        if let HQMGameObject::Puck(this_puck) = & mut self.game.world.objects[puck] {
                            if Some(this_connected_player_index) != this_puck.last_player_index[0] {
                                this_puck.last_player_index[3] = this_puck.last_player_index[2];
                                this_puck.last_player_index[2] = this_puck.last_player_index[1];
                                this_puck.last_player_index[1] = this_puck.last_player_index[0];
                                this_puck.last_player_index[0] = Some(this_connected_player_index);
                            }
                        }
                    }

                }
            }
        }
    }

    async fn send_update(&self, player: &HQMConnectedPlayer, i: usize, socket: & UdpSocket, packets: &[HQMObjectPacket], write_buf: & mut [u8]) {
        let mut writer = HQMServerWriter::new(write_buf);
        if player.game_id != self.game.game_id {
            writer.write_bytes_aligned(GAME_HEADER);
            writer.write_byte_aligned(6);
            writer.write_u32_aligned(self.game.game_id);
        } else {
            writer.write_bytes_aligned(GAME_HEADER);
            writer.write_byte_aligned(5);
            writer.write_u32_aligned(self.game.game_id);
            writer.write_u32_aligned(self.game.game_step);
            writer.write_bits(1, match self.game.game_over {
                true => 1,
                false => 0
            });
            writer.write_bits(8, self.game.red_score);
            writer.write_bits(8, self.game.blue_score);
            writer.write_bits(16, self.game.time);
            writer.write_bits(16, self.game.timeout);
            writer.write_bits(8, self.game.period);
            writer.write_bits(8, i as u32);

            // if using a non-cryptic version, send ping
            if player.client_version > 0 {
                writer.write_u32_aligned(player.deltatime);
            }

            // if baba's second version or above, send rules
            if player.client_version > 1 {
                writer.write_u32_aligned(self.game.rules_state.update_num());
            }

            writer.write_u32_aligned(self.game.packet);
            writer.write_u32_aligned(player.packet);

            for i in 0..32 {
                match &packets[i] {
                    HQMObjectPacket::Puck(puck) => {
                        writer.write_bits(1, 1);
                        writer.write_bits(2, 1); // Puck type
                        writer.write_pos(17, puck.pos.0);
                        writer.write_pos(17, puck.pos.1);
                        writer.write_pos(17, puck.pos.2);
                        writer.write_pos(31, puck.rot.0);
                        writer.write_pos(31, puck.rot.1);
                    } ,
                    HQMObjectPacket::Skater(skater) => {
                        writer.write_bits(1, 1);
                        writer.write_bits(2, 0); // Skater type
                        writer.write_pos(17, skater.pos.0);
                        writer.write_pos(17, skater.pos.1);
                        writer.write_pos(17, skater.pos.2);
                        writer.write_pos(31, skater.rot.0);
                        writer.write_pos(31, skater.rot.1);
                        writer.write_pos(13, skater.stick_pos.0);
                        writer.write_pos(13, skater.stick_pos.1);
                        writer.write_pos(13, skater.stick_pos.2);
                        writer.write_pos(25, skater.stick_rot.0);
                        writer.write_pos(25, skater.stick_rot.1);
                        writer.write_pos(16, skater.head_rot);
                        writer.write_pos(16, skater.body_rot);
                    },
                    HQMObjectPacket::None => {
                        writer.write_bits(1, 0);
                    }
                }
            }

            let remaining_messages = min(player.messages.len() - player.msgpos as usize, 15);

            writer.write_bits(4, remaining_messages as u32);
            writer.write_bits(16, player.msgpos);

            let pos2 = player.msgpos as usize;

            for i in pos2..pos2 + remaining_messages {
                let message = &player.messages[i];
                match Rc::as_ref(message) {
                    HQMMessage::Chat {
                        player_index,
                        message
                    } => {
                        writer.write_bits(6, 2);
                        writer.write_bits(6, match *player_index {
                            Some(x)=> x as u32,
                            None => u32::MAX
                        });
                        let size = min(63, message.len());
                        writer.write_bits(6, size as u32);
                        for i in 0..size {
                            writer.write_bits(7, message[i] as u32);
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
                        team,
                        player_index,
                        object_index,
                        in_server,
                    } => {
                        writer.write_bits(6, 0);
                        writer.write_bits(6, *player_index as u32);
                        writer.write_bits(1, if *in_server { 1 } else { 0 });
                        writer.write_bits(2, team.get_num());
                        writer.write_bits(6, match *object_index {
                            Some (x) => x as u32,
                            None => u32::MAX
                        });

                        for i in 0usize..31 {
                            let v = if i < player_name.len() {
                                player_name[i]
                            } else {
                                0
                            };
                            writer.write_bits(7, v as u32);
                        }
                    }
                };
            }
        }

        let slice = writer.get_slice();
        let _ = socket.send_to(slice, player.addr).await;
    }

    fn new_game(&mut self) {
        self.game_alloc += 1;
        self.game = HQMGame::new(self.game_alloc, &self.config);

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
                player.team = HQMTeam::Spec;
                player.msgpos = 0;
                player.packet = u32::MAX;
                player.messages.clear();
                let update = HQMMessage::PlayerUpdate {
                    player_name: player.player_name.clone().into_bytes(),
                    team: HQMTeam::Spec,
                    player_index: i,
                    object_index: None,
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

    fn do_faceoff(&mut self, faceoff_position_index: usize){

        // For making sure teams have centers
        let mut red_default_role_found = false;
        let mut blue_default_role_found = false;

        // Get amount of faceoff positiions, so we don't need to keep calling the function throughout
        let role_amount = self.config.faceoff_positions.len();

        // Make sure positions are good with no doubles (until there are more players than positions to fill)]
        // Creates a 2 (red,blue) by [amount of positions] boolean array
        let mut position_filled = vec![vec![0; role_amount]; 2];

        // Keeps track of the amount of players for a team that have been assigned
        // if this number is higher than the amount of positions/roles, preventing
        // multiples will be disregarded
        let mut red_players_found=0;
        let mut blue_players_found=0;

        // Loop through players
        for p in self.players.iter_mut() {
            if let Some(player) = p {
                if let Some(skater_obj_index) = player.skater {
                    if let HQMGameObject::Player(_) = & self.game.world.objects[skater_obj_index] {

                        // Check team's "position filled?" array, and if filled, find an untaken position
                        // otherwise, flip the "position filled?" flag
                        match player.team {
                            HQMTeam::Red => {
                                if position_filled[0][player.faceoff_position_index] == 1 {
                                    for this_position_index in 0..role_amount {
                                        if position_filled[0][this_position_index] != 1 {
                                            position_filled[0][this_position_index]=1;
                                            player.set_role(this_position_index);
                                            break;
                                        }
                                    }
                                } else {
                                    position_filled[0][player.faceoff_position_index] = 1;
                                }
                                red_players_found = red_players_found + 1;
                            },
                            HQMTeam::Blue => {
                                if position_filled[1][player.faceoff_position_index] == 1 {
                                    for this_position_index in 0..role_amount {
                                        if position_filled[1][this_position_index] != 1 {
                                            position_filled[1][this_position_index]=1;
                                            player.set_role(this_position_index);
                                            break;
                                        }
                                    }
                                }else{
                                    position_filled[1][player.faceoff_position_index] = 1;
                                }
                                blue_players_found = blue_players_found + 1
                            },
                            _ => {}
                        }

                        println!("{} position {}", player.player_name,player.faceoff_position_index);
                    }
                }
            }
        }


        // Make sure each team has a center
        for p in self.players.iter() {
            if let Some(player) = p {
                if let Some(skater_obj_index) = player.skater {
                    if let HQMGameObject::Player(_) = & self.game.world.objects[skater_obj_index] {
                        if player.faceoff_position_index == 0 {
                            match player.team {
                                HQMTeam::Red => {
                                    red_default_role_found=true;
                                },
                                HQMTeam::Blue => {
                                    blue_default_role_found=true;
                                },
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // One or more do not have a role
        if !red_default_role_found || !blue_default_role_found {
            for p in self.players.iter_mut() {
                if let Some(player) = p {
                    if let Some(skater_obj_index) = player.skater {
                        if let HQMGameObject::Player(_) = & self.game.world.objects[skater_obj_index] {

                            match player.team{
                                HQMTeam::Red => {
                                    if !red_default_role_found{
                                        player.faceoff_position_index = 0;
                                        break;
                                    }
                                },
                                HQMTeam::Blue =>{
                                    if !blue_default_role_found{
                                        player.faceoff_position_index = 0;
                                        break;
                                    }
                                },
                                _=>{

                                }
                            }
                        }

                        if red_default_role_found && blue_default_role_found{
                            break;
                        }
                    }
                }
            }
        }

        let mid = Point3::new (self.game.world.rink.width / 2.0, 0.0, self.game.world.rink.length / 2.0);

        self.game.world.objects = vec![HQMGameObject::None; 32];
        self.game.world.create_puck_object(Point3::new (mid.x, 1.5, mid.z), Matrix3::identity());

        let mut messages = Vec::new();

        fn setup (messages: & mut Vec<HQMMessage>, world: & mut HQMGameWorld,
                  player: & mut HQMConnectedPlayer, player_index: usize, pos: Point3<f32>, rot: Matrix3<f32>) {
            let new_object_index = world.create_player_object(pos, rot,
                                                              player.hand, player_index);
            player.skater = new_object_index;
            if new_object_index.is_none() {
                // Something very strange happened, we have to move the player
                // back to the spectators
                player.team = HQMTeam::Spec;
            }

            let update = HQMMessage::PlayerUpdate {
                player_name: player.player_name.clone().into_bytes(),
                team: player.team,
                player_index,
                object_index: new_object_index,
                in_server: true,
            };
            messages.push(update);
        }

        for (player_index, p) in self.players.iter_mut().enumerate() {
            if let Some(player) = p {
                let p = &self.config.faceoff_positions[player.faceoff_position_index].faceoff_offsets[faceoff_position_index];
                match player.team{
                    HQMTeam::Red=>{
                        let player_rotation = Rotation3::from_euler_angles(0.0,0.0,0.0);
                        let player_position = &mid + &player_rotation * p;
                        setup (& mut messages, & mut self.game.world, player, player_index, player_position,
                               player_rotation.matrix().clone_owned())
                    },
                    HQMTeam::Blue=>{
                        let player_rotation = Rotation3::from_euler_angles(0.0,std::f32::consts::PI,0.0);
                        let player_position = &mid + &player_rotation * p;
                        setup (& mut messages, & mut self.game.world, player, player_index, player_position,
                               player_rotation.matrix().clone_owned())
                    },
                    _=>{}
                }
            }
        }

        for message in messages {
            self.add_global_message(message, true);
        }

    }

    fn update_clock(&mut self) {
        if !self.game.paused {
            if self.game.period == 0 && self.game.time > 2000 {
                let mut has_red_players = false;
                let mut has_blue_players = false;
                for player in self.players.iter() {
                    if let Some(p) = player {
                        match p.team {
                            HQMTeam::Red => {
                                has_red_players = true;
                            },
                            HQMTeam::Blue => {
                                has_blue_players = true;
                            },
                            _ => {}
                        }
                    }
                    if has_red_players && has_blue_players {
                        self.game.time = 2000;
                        break;
                    }
                }
            }
            if self.game.game_over {
                self.game.intermission -= 1;
                if self.game.intermission == 0 {
                    self.new_game();
                }
            } else if self.game.timeout > 0 {
                self.game.timeout -= 1;
                if self.game.timeout == 0 && !self.game.game_over {
                    self.do_faceoff(0);
                }
            } else if self.game.time > 0 {
                self.game.time -= 1;
                if self.game.time == 0 {
                    self.game.period += 1;
                    self.game.intermission = self.config.time_intermission*100;
                }
            } else {
                if self.game.period > 3 && self.game.red_score != self.game.blue_score {
                    self.game.intermission = self.config.time_intermission*100;
                    self.game.game_over = true;
                } else {
                    self.game.intermission -= 1;
                    if self.game.intermission == 0 {
                        self.game.time = self.config.time_period*100;
                        self.do_faceoff(0);
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
        let mut read_buf = [0u8;1024];
        let mut write_buf = [0u8;4096];
        if self.config.public {
            let socket = socket.clone();
            tokio::spawn(async move {
                let mut public_timer = tokio::time::interval(Duration::from_secs(2));
                loop {
                    let _ = notify_master_server(&socket).await;
                    public_timer.tick().await;
                }
            });
        }
        loop {
            tokio::select! {
                _ = tick_timer.tick() => {
                    self.tick(& socket, & mut write_buf).await;
                }
                Ok((size, addr)) = socket.recv_from(&mut read_buf) => {
                    self.handle_message(addr, & socket, & read_buf[0..size], & mut write_buf).await;
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
            config
        }
    }
}

fn get_packets (objects: & Vec<HQMGameObject>) -> Vec<HQMObjectPacket> {
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
        Ok(s) => Some(s),
        Err(_) => None
    };
}

async fn notify_master_server(socket: & UdpSocket) -> std::io::Result<usize> {
    let server_addr: SocketAddr = MASTER_SERVER.parse().unwrap();
    let msg = b"Hock\x20";
    socket.send_to(msg, server_addr).await
}

struct HQMConnectedPlayer {
    player_name: String,
    addr: SocketAddr,
    client_version: u8,
    team: HQMTeam,
    faceoff_position_index: usize,
    skater: Option<usize>,
    game_id: u32,
    input: HQMPlayerInput,
    packet: u32,
    msgpos: u32,
    chat_rep: u32,
    messages: Vec<Rc<HQMMessage>>,
    inactivity: u32,
    is_admin: bool,
    is_muted:bool,
    team_switch_timer: u32,
    hand: HQMSkaterHand,
    deltatime: u32
}

impl HQMConnectedPlayer {
    pub fn new(player_name: String, addr: SocketAddr, global_messages: Vec<Rc<HQMMessage>>) -> Self {
        HQMConnectedPlayer {
            player_name,
            addr,
            client_version: 0,
            team: HQMTeam::Spec,
            faceoff_position_index: 0,
            skater: None,
            game_id: u32::MAX,
            packet: u32::MAX,
            msgpos: 0,
            chat_rep: 0,
            messages: global_messages,
            input: HQMPlayerInput::default(),
            inactivity: 0,
            is_admin: false,
            is_muted:false,
            hand: HQMSkaterHand::Right,
            team_switch_timer: 0,
            // store latest deltime client sends you to respond with it
            deltatime: 0
        }
    }

    pub fn set_role(&mut self,input_role:usize){
        self.faceoff_position_index = input_role;
    }
}


pub(crate) struct HQMServerConfiguration {
    pub(crate) server_name: String,
    pub(crate) port: u16,
    pub(crate) public: bool,
    pub(crate) player_max: u32,
    pub(crate) team_max: u32,
    pub(crate) force_team_size_parity: bool,
    pub(crate) welcome: Vec<String>,

    pub(crate) password: String,

    pub(crate) time_period: u32,
    pub(crate) time_warmup: u32,
    pub(crate) time_intermission: u32,
    pub(crate) warmup_pucks: u32,
    pub(crate) limit_jump_speed: bool,

    pub(crate) faceoff_positions: Vec<HQMFaceoffPosition>,

    pub(crate) entry_point_red: Vector3<f32>,
    pub(crate) entry_point_blue: Vector3<f32>,
    pub(crate) entry_rotation_red: f32,
    pub(crate) entry_rotation_blue: f32
}