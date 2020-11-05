use std::net::{SocketAddr};

use nalgebra::{Vector3, Point3, Matrix3, Vector2, Rotation3};

use std::cmp::min;
use std::time::Duration;
use std::path::Path;

// INI Crate For configuration
extern crate ini;
use ini::Ini;

mod hqm_parse;
mod hqm_simulate;

use hqm_parse::{HQMClientParser, HQMServerWriter, HQMObjectPacket};
use hqm_parse::{HQMPuckPacket, HQMSkaterPacket};
use tokio::net::UdpSocket;
use std::rc::Rc;
use std::env;
use crate::hqm_simulate::HQMSimulationEvent;
use std::fmt::{Display, Formatter};
use core::fmt;

const GAME_HEADER: &[u8] = b"Hock";

const MASTER_SERVER: &str = "66.226.72.227:27590";

struct HQMGame {

    state: HQMGameState,
    rules_state: HQMRulesState,
    objects: Vec<HQMGameObject>,
    global_messages: Vec<Rc<HQMMessage>>,
    red_score: u32,
    blue_score: u32,
    period: u32,
    time: u32,
    timeout: u32,
    intermission: u32,
    paused: bool,
    game_id: u32,
    game_step: u32,
    game_over: bool,
    packet: u32,
    rink: HQMRink,
    active: bool,
}

#[derive(Debug, Clone)]
struct HQMRinkNet {
    team: HQMTeam,
    posts: Vec<(Point3<f32>, Point3<f32>, f32)>,
    surfaces: Vec<(Point3<f32>,Point3<f32>,Point3<f32>,Point3<f32>)>,
    left_post: Point3<f32>,
    right_post: Point3<f32>,
    normal: Vector3<f32>,
    left_post_inside: Vector3<f32>,
    right_post_inside: Vector3<f32>
}

impl HQMRinkNet {
    fn new(team: HQMTeam, rink_width: f32, rink_length: f32) -> Self {
        let mid_x = rink_width / 2.0;

        let (pos, rot) = match team {
            HQMTeam::Blue => (Point3::new (mid_x, 0.0, 3.5), Matrix3::identity()),
            HQMTeam::Red => (Point3::new (mid_x, 0.0, rink_length - 3.5), Matrix3::from_columns (& [-Vector3::x(), Vector3::y(), -Vector3::z()])),
            _ => panic!()
        };
        let (front_upper_left, front_upper_right, front_lower_left, front_lower_right,
            back_upper_left, back_upper_right, back_lower_left, back_lower_right) =
            (
                &pos + &rot * Vector3::new(-1.5, 1.0, 0.5),
                &pos + &rot * Vector3::new(1.5, 1.0, 0.5),
                &pos + &rot * Vector3::new(-1.5, 0.0, 0.5),
                &pos + &rot * Vector3::new(1.5, 0.0, 0.5),
                &pos + &rot * Vector3::new(-1.25, 1.0, -0.25),
                &pos + &rot * Vector3::new(1.25, 1.0, -0.25),
                &pos + &rot * Vector3::new(-1.25, 0.0, -0.5),
                &pos + &rot * Vector3::new(1.25, 0.0, -0.5)
            );

        HQMRinkNet {
            team,
            posts: vec![
                (front_lower_right.clone(), front_upper_right.clone(), 0.1875),
                (front_lower_left.clone(), front_upper_left.clone(), 0.1875),
                (front_upper_right.clone(), front_upper_left.clone(), 0.125),

                (front_lower_left.clone(), back_lower_left.clone(), 0.125),
                (front_lower_right.clone(), back_lower_right.clone(), 0.125),
                (front_upper_left.clone(), back_upper_left.clone(), 0.125),
                (back_upper_right.clone(), front_upper_right.clone(), 0.125),

                (back_lower_left.clone(), back_upper_left.clone(), 0.125),
                (back_lower_right.clone(), back_upper_right.clone(), 0.125),
                (back_lower_left.clone(), back_lower_right.clone(), 0.125),
                (back_upper_left.clone(), back_upper_right.clone(), 0.125),

            ],
            surfaces: vec![
                (back_upper_left.clone(), back_upper_right.clone(),
                 back_lower_right.clone(), back_lower_left.clone()),
                (front_upper_left.clone(), back_upper_left.clone(),
                 back_lower_left.clone(), front_lower_left.clone()),
                (front_upper_right, front_lower_right.clone(),
                 back_lower_right.clone(), back_upper_right.clone()),
                (front_upper_left.clone(), front_upper_right.clone(),
                 back_upper_right.clone(), back_upper_left.clone())
            ],
            left_post: front_lower_left.clone(),
            right_post: front_lower_right.clone(),
            normal: rot * Vector3::z(),
            left_post_inside: rot * Vector3::x(),
            right_post_inside: rot * -Vector3::x()
        }

    }
}

#[derive(Debug, Clone)]
struct HQMRink {
    planes: Vec<(Point3<f32>, Vector3<f32>)>,
    corners: Vec<(Point3<f32>, Vector3<f32>, f32)>,
    nets: Vec<HQMRinkNet>,
    width:f32,
    length:f32
}

impl HQMRink {
    fn new(width: f32, length: f32, corner_radius: f32) -> Self {

        let zero = Point3::new(0.0,0.0,0.0);
        let planes = vec![
            (zero.clone(), Vector3::y()),
            (Point3::new(0.0, 0.0, length), -Vector3::z()),
            (zero.clone(), Vector3::z()),
            (Point3::new(width, 0.0, 0.0), -Vector3::x()),
            (zero.clone(), Vector3::x()),
        ];
        let r = corner_radius;
        let wr = width - corner_radius;
        let lr = length - corner_radius;
        let corners = vec![
            (Point3::new(r, 0.0, r),   Vector3::new(-1.0, 0.0, -1.0), corner_radius),
            (Point3::new(wr, 0.0, r),  Vector3::new( 1.0, 0.0, -1.0), corner_radius),
            (Point3::new(wr, 0.0, lr), Vector3::new( 1.0, 0.0,  1.0), corner_radius),
            (Point3::new(r, 0.0, lr),  Vector3::new(-1.0, 0.0,  1.0), corner_radius)
        ];
        let red_net = HQMRinkNet::new(HQMTeam::Red, width, length);
        let blue_net = HQMRinkNet::new(HQMTeam::Blue, width, length);
        HQMRink {
            planes,
            corners,
            nets: vec![red_net, blue_net],
            width,
            length
        }
    }
}

impl HQMGame {
    fn new (game_id: u32) -> Self {
        let mut object_vec = Vec::with_capacity(32);
        for _ in 0..32 {
            object_vec.push(HQMGameObject::None);
        }

        HQMGame {
            state:HQMGameState::Warmup,
            rules_state:HQMRulesState::None,
            objects: object_vec,
            global_messages: vec![],
            red_score: 0,
            blue_score: 0,
            period: 0,
            time: 30000,
            timeout: 0,
            intermission: 0,
            paused: false,

            game_over: false,
            game_id,
            game_step: 0,
            packet: 0,
            rink: HQMRink::new(30.0, 61.0, 8.5),
            active: false,
        }
    }
}

struct HQMServer {
    players: Vec<Option<HQMConnectedPlayer>>,
    config: HQMServerConfiguration,
    game: HQMGame,
    game_alloc: u32,
    is_muted:bool,
}

impl HQMServer {
    async fn handle_message(&mut self, addr: SocketAddr, socket: & UdpSocket, msg: &[u8], write_buf: & mut [u8]) {
        let mut parser = hqm_parse::HQMClientParser::new(&msg);
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
        let current_slot = HQMServer::find_player_slot(self, addr);
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
            }
        }
    }

    fn mute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=true;

                let msg = format!("Chat muted by {}",player.player_name);
                self.add_server_chat_message(msg);
            }
        }
    }

    fn unmute_chat (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.is_muted=false;

                let msg = format!("Chat unmuted by {}",player.player_name);
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
            }
        }
    }

    fn set_clock (& mut self, input_minutes: u32, input_seconds: u32, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.time = (input_minutes * 60 * 100)+ (input_seconds * 100);

                let msg = format!("Clock set by {}", player.player_name);
                self.add_server_chat_message(msg);
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
            }
        }
    }

    fn set_period (& mut self, input_period: u32,player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{

                self.game.period = input_period;
                
                let msg = format!("Period set by {}",player.player_name);
                self.add_server_chat_message(msg);

            }
        }
    }

    fn faceoff (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.timeout = 5*100;

                let msg = format!("Faceoff initiated by {}",player.player_name);
                self.add_server_chat_message(msg);
            }
        }
    }

    fn reset_game (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                let msg = format!("Game reset by {}",player.player_name);

                self.new_game();

                self.add_server_chat_message(msg);
            }
        }
    }

    fn pause (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.paused=true;

                let msg = format!("Game paused by {}",player.player_name);
                self.add_server_chat_message(msg);
            }
        }
    }

    fn unpause (& mut self, player_index: usize) {
        if let Some(player) = & self.players[player_index] {
            if player.is_admin{
                self.game.paused=false;

                let msg = format!("Game resumed by {}",player.player_name);
                self.add_server_chat_message(msg);
            }
        }
    }

    fn set_hand (& mut self, hand: HQMSkaterHand, player_index: usize) {

        
        if let Some(player) = & mut self.players[player_index] {
            player.hand = hand;
            if let Some(skater_obj_index) = player.skater {
                if let HQMGameObject::Player(skater) = & mut self.game.objects[skater_obj_index] {

                    if self.game.state == HQMGameState::Game {

                        let msg = format!("{}: You can not change your stick hand in-game",player.player_name);
                        self.add_server_chat_message(msg);

                        return;
                    }

                    skater.hand = hand;
                }
            }
        }
        
    }

    fn process_command (&mut self, command: &str, args: &[&str], player_index: usize) {

        match command{
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
                let split: Vec<&str> = msg.split_ascii_whitespace().collect();
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
        let current_slot = HQMServer::find_player_slot(self, addr);
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
        let player_index = HQMServer::find_empty_player_slot(self);
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
                    self.game.objects[object_index] = HQMGameObject::None;
                }

                self.add_global_message(update, true);

                self.players[player_index as usize] = None;
            }
            None => {
                println!("Player has already exited");
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
        let current_slot = HQMServer::find_player_slot(self, addr);
        if current_slot.is_some() {
            return; // Player has already joined
        }

        let player_name_bytes = parser.read_bytes_aligned(32);
        let player_name = HQMServer::get_player_name(player_name_bytes);
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

    fn find_empty_object_slot(objects: & Vec<HQMGameObject>) -> Option<usize> {
        return objects.iter().position(|x| {match x {
            HQMGameObject::None  => true,
            _ => false
        }});
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

    fn create_player_object (objects: & mut Vec<HQMGameObject>, start: Point3<f32>, rot: Matrix3<f32>, hand: HQMSkaterHand, connected_player_index: usize) -> Option<usize> {
        let object_slot = HQMServer::find_empty_object_slot(& objects);
        if let Some(i) = object_slot {
            objects[i] = HQMGameObject::Player(HQMSkater::new(i, start, rot, hand, connected_player_index));
        }
        return object_slot;
    }

    fn create_puck_object (objects: & mut Vec<HQMGameObject>, start: Point3<f32>, rot: Matrix3<f32>) -> Option<usize> {
        let object_slot = HQMServer::find_empty_object_slot(& objects);
        if let Some(i) = object_slot {
            objects[i] = HQMGameObject::Puck(HQMPuck::new(i, start, rot));
        }
        return object_slot;
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
                    let (new_team, new_team_count) = if player.input.join_red() {
                        (HQMTeam::Red, & mut red_player_count)
                    } else {
                        (HQMTeam::Blue, & mut blue_player_count)
                    };
                    if new_team != player.team && *new_team_count + 1 <= self.config.team_max {
                        if player.skater.is_none() {
                            let (mid_x, mid_z) = (self.game.rink.width / 2.0, self.game.rink.length / 2.0);
                            let pos = Point3::new(mid_x, 2.5, mid_z);
                            let rot = Matrix3::identity();

                            if let Some(i) = HQMServer::create_player_object(& mut self.game.objects, pos, rot, player.hand, player_index) {
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
                        })
                    }
                } else if player.input.spectate() && player.team != HQMTeam::Spec {
                    player.team = HQMTeam::Spec;
                    player.team_switch_timer = 500; // 500 ticks, 5 seconds
                    if let Some (i) = player.skater {
                        self.game.objects[i] = HQMGameObject::None;
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
                    if let HQMGameObject::Player(player_object) = & mut self.game.objects[object_index] {
                        player_object.input = player.input.clone();
                    }
                }
            }
        }
    }

    async fn tick(&mut self, socket: & UdpSocket, write_buf: & mut [u8]) {
        if self.player_count() != 0 {
            self.game.active = true;
            self.remove_inactive_players ();
            self.move_players_between_teams();
            self.copy_player_input_to_object();
            let events = self.game.simulate_step();
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
                            
                            if let HQMGameObject::Puck(this_puck) = & mut self.game.objects[puck] {
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
                        if let HQMGameObject::Player(this_skater) = & mut self.game.objects[player] {
                            let this_connected_player_index = this_skater.connected_player_index;

                            // Store player index in queue for awarding goals/assists
                            if let HQMGameObject::Puck(this_puck) = & mut self.game.objects[puck] {
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
            self.update_clock();

            self.update_game_state();

            let mut packets: Vec<HQMObjectPacket> = Vec::with_capacity(32);
            for i in 0usize..32 {
                let packet = match &self.game.objects[i] {
                    HQMGameObject::Puck(puck) => HQMObjectPacket::Puck(puck.get_packet()),
                    HQMGameObject::Player(player) => HQMObjectPacket::Skater(player.get_packet()),
                    HQMGameObject::None => HQMObjectPacket::None
                };
                packets.push(packet);
            }

            for (i, x) in self.players.iter().enumerate() {
                if let Some(p) = x {
                    self.send_update(p, i, socket, &packets, write_buf).await;
                }
            }
            self.game.packet += 1;
            self.game.game_step += 1;
        } else if self.game.active {
            self.new_game();
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
        self.game = HQMGame::new(self.game_alloc);

        let puck_line_start= self.game.rink.width / 2.0 - 0.4 * ((self.config.warmup_pucks - 1) as f32);

        for i in 0..self.config.warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8*(i as f32), 1.5, self.game.rink.length / 2.0);
            let rot = Matrix3::identity();
            HQMServer::create_puck_object(& mut self.game.objects, pos, rot);
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
                    if let HQMGameObject::Player(_) = & self.game.objects[skater_obj_index] {

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
                    if let HQMGameObject::Player(_) = & self.game.objects[skater_obj_index] {
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
                        if let HQMGameObject::Player(_) = & self.game.objects[skater_obj_index] {

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

        let mid = Point3::new (self.game.rink.width / 2.0, 0.0, self.game.rink.length / 2.0);

        self.game.objects = vec![HQMGameObject::None; 32];
        HQMServer::create_puck_object(& mut self.game.objects, Point3::new (mid.x, 1.5, mid.z), Matrix3::identity());

        let mut messages = Vec::new();

        fn setup (messages: & mut Vec<HQMMessage>, objects: & mut Vec<HQMGameObject>,
                  player: & mut HQMConnectedPlayer, player_index: usize, pos: Point3<f32>, rot: Matrix3<f32>) {
            let new_object_index = HQMServer::create_player_object(objects,
                                                                   pos, rot,
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
                        setup (& mut messages, & mut self.game.objects, player, player_index, player_position,
                               player_rotation.matrix().clone_owned())
                    },
                    HQMTeam::Blue=>{
                        let player_rotation = Rotation3::from_euler_angles(0.0,std::f32::consts::PI,0.0);
                        let player_position = &mid + &player_rotation * p;
                        setup (& mut messages, & mut self.game.objects, player, player_index, player_position,
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

    fn update_game_state(&mut self){
        
        if !self.game.paused {

            if self.game.period == 0 {
                self.game.state = HQMGameState::Warmup;
            } else {
                self.game.state = HQMGameState::Game;
            }

            if self.game.intermission > 0 {
                self.game.state = HQMGameState::Intermission;
            }

            if self.game.timeout > 0 {
                self.game.state = HQMGameState::Timeout;
            }

            if self.game.game_over {
                self.game.state = HQMGameState::GameOver;
            }

            

        } else{
            self.game.state = HQMGameState::Paused;
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
        let mut public_timer = tokio::time::interval(Duration::from_secs(2));

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let socket = tokio::net::UdpSocket::bind(& addr).await?;
        let mut read_buf = [0u8;1024];
        let mut write_buf = [0u8;4096];
        loop {
            tokio::select! {
                _ = tick_timer.tick() => {
                    self.tick(& socket, & mut write_buf).await;
                }
                _ = public_timer.tick(), if self.config.public => {
                    let _ = notify_master_server(& socket).await;
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
            game: HQMGame::new(1),
            game_alloc: 1,
            is_muted:false,
            config
        }
    }
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

#[derive(Debug, Clone)]
struct HQMPlayerInput {
    stick_angle: f32,
    turn: f32,
    unknown: f32,
    fwbw: f32,
    stick: Vector2<f32>,
    head_rot: f32,
    body_rot: f32,
    keys: u32,
}

impl Default for HQMPlayerInput {
    fn default() -> Self {
        HQMPlayerInput {
            stick_angle: 0.0,
            turn: 0.0,
            unknown: 0.0,
            fwbw: 0.0,
            stick: Vector2::new(0.0, 0.0),
            head_rot: 0.0,
            body_rot: 0.0,
            keys: 0
        }
    }
}

impl HQMPlayerInput {
    pub fn jump (&self) -> bool { self.keys & 0x1 != 0}
    pub fn crouch (&self) -> bool { self.keys & 0x2 != 0}
    pub fn join_red (&self) -> bool { self.keys & 0x4 != 0}
    pub fn join_blue (&self) -> bool { self.keys & 0x8 != 0}
    pub fn shift (&self) -> bool { self.keys & 0x10 != 0}
    pub fn spectate (&self) -> bool { self.keys & 0x20 != 0}
}

#[derive(Debug, Clone)]
enum HQMGameObject {
    None,
    Player(HQMSkater),
    Puck(HQMPuck),
}

struct HQMFaceoffPosition {
    abbreviation: String,
    faceoff_offsets: Vec<Vector3<f32>> // To store multiple faceoff positions as needed
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HQMTeam {
    Spec,
    Red,
    Blue,
}

impl HQMTeam {
    fn get_num(self) -> u32 {
        match self {
            HQMTeam::Red => 0,
            HQMTeam::Blue => 1,
            HQMTeam::Spec => u32::MAX
        }
    }
}

impl Display for HQMTeam {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HQMTeam::Red => write!(f, "Red"),
            HQMTeam::Blue => write!(f, "Blue"),
            HQMTeam::Spec => write!(f, "Spec")
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HQMGameState {
    Warmup,
    Game,
    Intermission,
    Timeout,
    Paused,
    GameOver,
}

impl Display for HQMGameState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HQMGameState::Warmup => write!(f, "Warmup"),
            HQMGameState::Game => write!(f, "Game"),
            HQMGameState::Intermission => write!(f, "Intermission"),
            HQMGameState::Timeout => write!(f, "Timeout"),
            HQMGameState::Paused => write!(f, "Paused"),
            HQMGameState::GameOver => write!(f, "Game Over"),

        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HQMRulesState {
    None,
    OffsideWarning,
    IcingWarning,
    DualWarning,
    Offside,
    Icing,
}

impl HQMRulesState {
    fn update_num(self) -> u32 {
        match self {
            HQMRulesState::None => 0,
            HQMRulesState::OffsideWarning => 1,
            HQMRulesState::IcingWarning => 2,
            HQMRulesState::DualWarning => 3,
            HQMRulesState::Offside => 4,
            HQMRulesState::Icing => 8,

        }
    }
}

impl Display for HQMRulesState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HQMRulesState::None => write!(f, "None"),
            HQMRulesState::OffsideWarning => write!(f, "Offside Warning"),
            HQMRulesState::IcingWarning => write!(f, "Icing Warning"),
            HQMRulesState::DualWarning => write!(f, "Offside Warning + Icing Warning"),
            HQMRulesState::Offside => write!(f, "Offside"),
            HQMRulesState::Icing => write!(f, "Icing"),

        }
    }
}

#[derive(Debug, Clone)]
struct HQMBody {
    pos: Point3<f32>,                // Measured in meters
    linear_velocity: Vector3<f32>,   // Measured in meters per hundred of a second
    rot: Matrix3<f32>,               // Rotation matrix
    angular_velocity: Vector3<f32>,  // Measured in radians per hundred of a second
    rot_mul: Vector3<f32>
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum HQMSkaterHand {
    Left, Right
}

#[derive(Debug, Clone)]
struct HQMSkater {
    index: usize,
    connected_player_index: usize,
    body: HQMBody,
    stick_pos: Point3<f32>,        // Measured in meters
    stick_velocity: Vector3<f32>,  // Measured in meters per hundred of a second
    stick_rot: Matrix3<f32>,       // Rotation matrix
    head_rot: f32,                 // Radians
    body_rot: f32,                 // Radians
    height: f32,
    input: HQMPlayerInput,
    old_input: HQMPlayerInput,
    stick_placement: Vector2<f32>,      // Azimuth and inclination in radians
    stick_placement_delta: Vector2<f32>, // Change in azimuth and inclination per hundred of a second
    collision_balls: Vec<HQMSkaterCollisionBall>,
    hand: HQMSkaterHand
}

impl HQMSkater {

    fn get_collision_balls(pos: &Point3<f32>, rot: &Matrix3<f32>, linear_velocity: &Vector3<f32>) -> Vec<HQMSkaterCollisionBall> {
        let mut collision_balls = Vec::with_capacity(6);
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.0, 0.0, 0.0), pos, rot, linear_velocity, 0.225));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.25, 0.3125, 0.0), pos, rot, linear_velocity, 0.25));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(-0.25, 0.3125, 0.0), pos, rot, linear_velocity, 0.25));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(-0.1875, -0.1875, 0.0), pos, rot, linear_velocity, 0.1875));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.1875, -0.1875, 0.0), pos, rot, linear_velocity, 0.1875));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.0, 0.5, 0.0), pos, & rot, linear_velocity, 0.1875));
        collision_balls
    }

    fn new(object_index: usize, pos: Point3<f32>, rot: Matrix3<f32>, hand: HQMSkaterHand, connected_player_index: usize) -> Self {
        let linear_velocity = Vector3::new (0.0, 0.0, 0.0);
        let collision_balls = HQMSkater::get_collision_balls(&pos, &rot, &linear_velocity);
        HQMSkater {
            index:object_index,
            connected_player_index,
            body: HQMBody {
                pos: pos.clone(),
                linear_velocity,
                rot,
                angular_velocity: Vector3::new (0.0, 0.0, 0.0),
                rot_mul: Vector3::new (2.75, 6.16, 2.35)
            },
            stick_pos: pos.clone(),
            stick_velocity: Vector3::new (0.0, 0.0, 0.0),
            stick_rot: Matrix3::identity(),
            head_rot: 0.0,
            body_rot: 0.0,
            height: 0.75,
            input: HQMPlayerInput::default(),
            old_input: HQMPlayerInput::default(),
            stick_placement: Vector2::new(0.0, 0.0),
            stick_placement_delta: Vector2::new(0.0, 0.0),
            hand,
            collision_balls
        }
    }

    fn get_packet(&self) -> HQMSkaterPacket {
        let rot = hqm_parse::convert_matrix(31, & self.body.rot);
        let stick_rot = hqm_parse::convert_matrix(25, & self.stick_rot);

        HQMSkaterPacket {
            pos: (get_position (17, 1024.0 * self.body.pos.x),
                  get_position (17, 1024.0 * self.body.pos.y),
                  get_position (17, 1024.0 * self.body.pos.z)),
            rot,
            stick_pos: (get_position (13, 1024.0 * (self.stick_pos.x - self.body.pos.x + 4.0)),
                        get_position (13, 1024.0 * (self.stick_pos.y - self.body.pos.y + 4.0)),
                        get_position (13, 1024.0 * (self.stick_pos.z - self.body.pos.z + 4.0))),
            stick_rot,
            head_rot: get_position (16, (self.head_rot + 2.0) * 8192.0),
            body_rot: get_position (16, (self.body_rot + 2.0) * 8192.0)
        }
    }

}

#[derive(Debug, Clone)]
struct HQMSkaterCollisionBall {
    offset: Vector3<f32>,
    pos: Point3<f32>,
    velocity: Vector3<f32>,
    radius: f32

}

impl HQMSkaterCollisionBall {
    fn from_skater(offset: Vector3<f32>, skater_pos: & Point3<f32>, skater_rot: & Matrix3<f32>, velocity: & Vector3<f32>, radius: f32) -> Self {
        let pos = skater_pos + skater_rot * offset;
        HQMSkaterCollisionBall {
            offset,
            pos,
            velocity: velocity.clone_owned(),
            radius
        }
    }
}

#[derive(Debug, Clone)]
struct HQMPuck {
    index: usize,
    body: HQMBody,
    radius: f32,
    height: f32,
    last_player_index: [Option<usize>; 4],
}

fn get_position (bits: u32, v: f32) -> u32 {
    let temp = v as i32;
    if temp < 0 {
        0
    } else if temp > ((1 << bits) - 1) {
        ((1 << bits) - 1) as u32
    } else {
        temp as u32
    }
}

impl HQMPuck {
    fn new(object_index:usize,pos: Point3<f32>, rot: Matrix3<f32>) -> Self {
        HQMPuck {
            index:object_index,
            body: HQMBody {
                pos,
                linear_velocity: Vector3::new(0.0, 0.0, 0.0),
                rot,
                angular_velocity: Vector3::new(0.0,0.0,0.0),
                rot_mul: Vector3::new(223.5, 128.0, 223.5)
            },
            radius: 0.125,
            height: 0.0412500016391,
            last_player_index: [None; 4]
        }
    }

    fn get_packet(&self) -> HQMPuckPacket {
        let rot = hqm_parse::convert_matrix(31, & self.body.rot);
        HQMPuckPacket {
            pos: (get_position (17, 1024.0 * self.body.pos.x),
                  get_position (17, 1024.0 * self.body.pos.y),
                  get_position (17, 1024.0 * self.body.pos.z)),
            rot
        }
    }

}


#[derive(Debug, Clone)]
enum HQMMessage {
    PlayerUpdate {
        player_name: Vec<u8>,
        team: HQMTeam,
        player_index: usize,
        object_index: Option<usize>,
        in_server: bool,
    },
    Goal {
        team: HQMTeam,
        goal_player_index: Option<usize>,
        assist_player_index: Option<usize>,
    },
    Chat {
        player_index: Option<usize>,
        message: Vec<u8>,
    },
}

struct HQMServerConfiguration {
    server_name: String,
    port: u16,
    public: bool,
    player_max: u32,
    team_max: u32,
    welcome: Vec<String>,
    
    password: String,

    time_period: u32,
    time_warmup: u32,
    time_intermission: u32,
    warmup_pucks: u32,

    faceoff_positions: Vec<HQMFaceoffPosition>,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = env::args().collect();

    let config_path = if args.len() > 1 {
        &args[1]
    } else {
        "config.ini"
    };
    // Init vec for roles
    let mut rolevec:Vec<HQMFaceoffPosition>=Vec::new();

    // Load configuration (if exists)
    let config = if Path::new(config_path).exists(){

        // Load configuration file
        let conf = Ini::load_from_file(config_path).unwrap();

        // Server information
        let server_section = conf.section(Some("Server")).unwrap();
        let server_name = server_section.get("name").unwrap().parse::<String>().unwrap();
        let server_port = server_section.get("port").unwrap().parse::<u16>().unwrap();
        let server_public = server_section.get("public").unwrap().parse::<bool>().unwrap();
        let server_player_max = server_section.get("player_max").unwrap().parse::<u32>().unwrap();
        let server_team_max = server_section.get("team_max").unwrap().parse::<u32>().unwrap();
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


        // Roles
        let roles_section = conf.section(Some("Roles")).unwrap();
        for (k, v) in roles_section.iter() {
            let string_abbreviation = k.parse::<String>().unwrap();
            let string_offsets = v.parse::<String>().unwrap();

            let mut offsets:Vec<Vector3<f32>>=Vec::new();

            let offset_parts: Vec<&str> = string_offsets.split('|').collect();
            for this_offset in offset_parts{
                let offset_parts: Vec<&str> = this_offset.split(',').collect();

                offsets.push(Vector3::new(offset_parts[0].parse::<f32>().unwrap(),
                                         offset_parts[1].parse::<f32>().unwrap(),
                                         offset_parts[2].parse::<f32>().unwrap()));
            }

            rolevec.push(HQMFaceoffPosition {
                abbreviation:string_abbreviation,
                faceoff_offsets:offsets
            });
        }

        HQMServerConfiguration {
            server_name,
            port: server_port,
            team_max: server_team_max,
            player_max: server_player_max,
            public: server_public,

            password: server_password,

            time_period: rules_time_period, 
            time_warmup: rules_time_warmup, 
            time_intermission: rules_time_intermission,
            warmup_pucks,

            faceoff_positions: rolevec,
            welcome: welcome_str
        }
    } else{

        // No config file: set defaults

        // Default roles
        rolevec.push(HQMFaceoffPosition {
            abbreviation: String::from("C"),
            faceoff_offsets:vec![Vector3::new(0.0,1.5,0.75)]
        });

        rolevec.push(HQMFaceoffPosition {
            abbreviation: String::from("LD"),
            faceoff_offsets:vec![Vector3::new(-2.0,1.5,8.0)]
        });

        rolevec.push(HQMFaceoffPosition {
            abbreviation: String::from("RD"),
            faceoff_offsets:vec![Vector3::new(2.0,1.5,8.0)]
        });

        rolevec.push(HQMFaceoffPosition {
            abbreviation: String::from("LW"),
            faceoff_offsets:vec![Vector3::new(-5.0,1.5,2.0)]
        });

        rolevec.push(HQMFaceoffPosition {
            abbreviation: String::from("RW"),
            faceoff_offsets:vec![Vector3::new(5.0,1.5,2.0)]
        });

        rolevec.push(HQMFaceoffPosition {
            abbreviation: String::from("G"),
            faceoff_offsets:vec![Vector3::new(0.0,1.5,22.0)]
        });

        // Default values
        HQMServerConfiguration {
            server_name: String::from("MigoTest"),
            port: 27585,
            public: true,

            team_max: 5,
            player_max: 15, // Codemonster TODO: implement

            password: String::from("admin"),

            time_period: 300,
            time_warmup: 300,
            time_intermission: 10,

            warmup_pucks: 1,
            faceoff_positions: rolevec,
            welcome: vec![]
        }

    };

    // Config file didn't exist; use defaults as described
    return HQMServer::new(config).run().await;

}

