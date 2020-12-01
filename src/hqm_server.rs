use std::net::{SocketAddr};

use nalgebra::{Vector3, Point3, Matrix3, Vector2, Rotation3};

use std::cmp::min;
use std::time::Duration;

use crate::hqm_parse::{HQMMessageReader, HQMMessageWriter, HQMObjectPacket};
use crate::hqm_simulate::HQMSimulationEvent;
use crate::hqm_game::{HQMTeam, HQMGameObject, HQMGameState, HQMSkaterHand, HQMGameWorld, HQMMessage, HQMGame, HQMPlayerInput, HQMIcingStatus, HQMOffsideStatus, HQMRulesState, HQMFaceoffSpot, HQMPuckTouch, HQMRink, HQMPuck};
use tokio::net::UdpSocket;
use std::rc::Rc;
use std::collections::{HashSet, HashMap};
use std::sync::Arc;


const GAME_HEADER: &[u8] = b"Hock";

const MASTER_SERVER: &str = "66.226.72.227:27590";

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
    async fn handle_message(&mut self, addr: SocketAddr, socket: & UdpSocket, msg: &[u8], write_buf: & mut [u8]) {
        let mut parser = HQMMessageReader::new(&msg);
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

    async fn request_info<'a>(&self, socket: & UdpSocket, addr: &SocketAddr, parser: &mut HQMMessageReader<'a>, write_buf: & mut [u8]) -> std::io::Result<usize> {
        let _player_version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();

        let mut writer = HQMMessageWriter::new(write_buf);
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

    fn player_update(&mut self, addr: &SocketAddr, parser: &mut HQMMessageReader, command: u8) {
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
        if packet < player.known_packet && player.known_packet - packet < 1000 {
            // UDP does not guarantee that the packets arrive in the same order they were sent,
            // or at all. This should prevent packets that are older than the most recent one
            // received from being applied.
            return;
        }

        player.inactivity = 0;
        player.known_packet = packet;
        player.input = input;
        player.game_id = current_game_id;
        player.known_msgpos = parser.read_u16_aligned();


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

    fn player_join(&mut self, addr: &SocketAddr, parser: &mut HQMMessageReader) {
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
                                _=>{}
                            }
                        },
                        _ => {}
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
                            Some(Some((skater_index, team)))
                        } else {
                            None
                        }
                    },
                    None => {
                        player.team_switch_timer = 500; // 500 ticks, 5 seconds

                        world.objects[skater_index] = HQMGameObject::None;
                        player.skater = None;
                        Some(None)
                    }
                }
            },
            None => {
                match team {
                    Some(team) => {

                        let (pos, rot) = match team {
                            HQMTeam::Red=>{
                                let pos = Point3::new(config.entry_point_red[0],config.entry_point_red[1],config.entry_point_red[2]);
                                let rot = Rotation3::from_euler_angles(0.0,config.entry_rotation_red,0.0);
                                (pos, rot)
                            },
                            HQMTeam::Blue=>{
                                let pos = Point3::new(config.entry_point_blue[0],config.entry_point_blue[1],config.entry_point_blue[2]);
                                let rot = Rotation3::from_euler_angles(0.0,config.entry_rotation_blue,0.0);
                                (pos, rot)
                            },
                        };

                        if let Some(i) = world.create_player_object(team, pos, rot.matrix().clone_owned(), player.hand, player_index) {
                            player.skater = Some(i);
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

    pub(crate) fn set_team (& mut self, player_index: usize, team: Option<HQMTeam>) -> Option<Option<(usize, HQMTeam)>> {
        match & mut self.players[player_index as usize] {
            Some(player) => {
                let res = HQMServer::set_team_internal(player_index, player, & mut self.game.world, & self.config, team);
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

    fn add_player(&mut self, player_name: String, addr: &SocketAddr) -> bool {
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

                let mut messages = self.game.global_messages.clone();
                for welcome_msg in self.config.welcome.iter() {
                    messages.push(Rc::new(HQMMessage::Chat {
                        player_index: None,
                        message: welcome_msg.clone()
                    }));
                }

                let new_player = HQMConnectedPlayer::new(player_name, *addr, messages);

                self.players[player_index] = Some(new_player);

                true
            }
            _ => false
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
                message,
            };
            self.add_global_message(chat, false);
        }

    }

    pub(crate) fn add_server_chat_message(&mut self, message: String) {
        println!("{}", &message);
        let chat = HQMMessage::Chat {
            player_index: None,
            message,
        };
        self.add_global_message(chat, false);
    }

    pub(crate) fn add_directed_server_chat_message(&mut self, message: String, player_receiving_index: usize) {
        println!("{}", &message);
        // This message will only be visible to a single player
        let chat = HQMMessage::Chat {
            player_index: None,
            message,
        };
        if let Some(player) = & mut self.players[player_receiving_index] {
            player.messages.push(Rc::new (chat));
        }
    }

    pub(crate) fn add_global_message(&mut self, message: HQMMessage, persistent: bool) {
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
        for p in self.game.world.objects.iter () {
            if let HQMGameObject::Player(player) = p {
                if player.team == HQMTeam::Red {
                    red_player_count += 1;
                } else if player.team == HQMTeam::Blue {
                    blue_player_count += 1;
                }
            }
        }
        let mut new_messages = Vec::new();
        for (player_index, player) in self.players.iter_mut().enumerate() {
            if let Some(player) = player {
                player.team_switch_timer = player.team_switch_timer.saturating_sub(1);
                let new_team = if (player.input.join_red() || player.input.join_blue())
                    && player.skater.is_none()
                    && player.team_switch_timer == 0 {
                    let (new_team, new_team_count, other_team_count) = if player.input.join_red() {
                        (HQMTeam::Red, & mut red_player_count, blue_player_count)
                    } else {
                        (HQMTeam::Blue, & mut blue_player_count, red_player_count)
                    };
                    if *new_team_count + 1 <= self.config.team_max
                        && (!self.config.force_team_size_parity || (*new_team_count <= other_team_count)) {
                        let has_skater = player.skater.is_some();
                        let res = HQMServer::set_team_internal(player_index, player, & mut self.game.world, & self.config, Some(new_team));
                        if res.is_some() && !has_skater {
                            *new_team_count += 1;
                        }
                        res
                    } else {
                        None
                    }
                } else if player.input.spectate() {
                    HQMServer::set_team_internal(player_index, player, & mut self.game.world, & self.config, None)
                } else {
                    None
                };
                if let Some(object) = new_team {
                    let msg = HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone(),
                        object,
                        player_index,
                        in_server: true
                    };
                    new_messages.push(msg);
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
            self.remove_inactive_players(); // connected players and objects
            self.move_players_between_teams();
            self.copy_player_input_to_object();
            let events = self.game.world.simulate_step();
            if self.config.mode == HQMServerMode::Match {
                self.handle_events(events);
                self.update_clock();
            }

            self.game.update_game_state();

            let packets = get_packets(& self.game.world.objects);

            for (player_index, player) in self.players.iter().enumerate() {
                if let Some(player) = player {
                    Self::send_update(&self.game, player, player_index, socket, &packets, write_buf).await;
                }
            }
            self.game.packet += 1;
            self.game.game_step += 1;
        } else if self.game.active {
            self.new_game();
            self.allow_join=true;
        }

    }

    fn call_goal (& mut self, team: HQMTeam, puck: usize) {
        if team == HQMTeam::Red {
            self.game.red_score += 1;
        } else if team == HQMTeam::Blue {
            self.game.blue_score += 1;
        }
        self.game.goal_timer = self.config.time_intermission*100;
        self.game.next_faceoff_spot = self.game.world.rink.center_faceoff_spot.clone();
        if self.game.period > 3 && self.game.red_score != self.game.blue_score {
            self.game.intermission = 2000;
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
        let offside_status = match team {
            HQMTeam::Red => & mut self.game.red_offside_status,
            HQMTeam::Blue => & mut self.game.blue_offside_status,
        };
        self.game.next_faceoff_spot = Self::get_offside_faceoff_spot(pass_origin, &self.game.world.rink, team);
        self.game.intermission = self.config.time_intermission*100;
        *offside_status = HQMOffsideStatus::Offside;
        self.add_server_chat_message(String::from("Offside"));
    }

    fn call_icing(& mut self, team: HQMTeam, pass_origin: &Point3<f32>) {
        let icing_status = match team {
            HQMTeam::Red => & mut self.game.red_icing_status,
            HQMTeam::Blue => & mut self.game.blue_icing_status,
        };
        self.game.next_faceoff_spot = Self::get_icing_faceoff_spot(pass_origin, &self.game.world.rink, team);
        self.game.intermission = self.config.time_intermission*100;
        *icing_status = HQMIcingStatus::Icing;
        self.add_server_chat_message(String::from("Icing"));
    }

    fn handle_events (& mut self, events: Vec<HQMSimulationEvent>) {
        if self.game.red_offside_status == HQMOffsideStatus::Offside
            || self.game.blue_offside_status == HQMOffsideStatus::Offside
            || self.game.red_icing_status == HQMIcingStatus::Icing
            || self.game.blue_icing_status == HQMIcingStatus::Icing
        || self.game.period == 0
        || self.game.time == 0
        || self.game.goal_timer > 0
        || self.game.intermission > 0 {
            return;
        }
        for event in events {
            match event {
                HQMSimulationEvent::PuckEnteredNet {
                    team, puck
                } => {
                    let offside_status = match team {
                        HQMTeam::Red => & mut self.game.red_offside_status,
                        HQMTeam::Blue => & mut self.game.blue_offside_status,
                    };
                    if *offside_status == HQMOffsideStatus::No {
                        self.call_goal(team, puck);
                    } else if let HQMOffsideStatus::Warning(p, _) = offside_status {
                        let copy = p.clone();
                        self.call_offside(team, &copy);
                    }
                },
                HQMSimulationEvent::PuckTouch {
                    player, puck
                } => {
                    // Get connected player index from skater
                    if let HQMGameObject::Player(skater) = & mut self.game.world.objects[player] {
                        let this_connected_player_index = skater.connected_player_index;
                        let team = skater.team;

                        if let HQMGameObject::Puck(puck) = & mut self.game.world.objects[puck] {
                            Self::update_puck_touches(puck, this_connected_player_index, team, self.game.time);

                            let (icing_status, other_icing_status, offside_status, other_team) = match team {
                                HQMTeam::Red => (& mut self.game.red_icing_status, & mut self.game.blue_icing_status, & mut self.game.red_offside_status, HQMTeam::Blue),
                                HQMTeam::Blue => (& mut self.game.blue_icing_status, & mut self.game.red_icing_status, & mut self.game.blue_offside_status, HQMTeam::Red),
                            };

                            if let HQMOffsideStatus::Warning(p, i) = offside_status {
                                let pass_origin = if this_connected_player_index == *i {
                                    puck.body.pos.clone()
                                } else {
                                    p.clone()
                                };
                                self.call_offside(team, &pass_origin);
                            } else if let HQMIcingStatus::Warning(p) = other_icing_status {
                                let copy = p.clone();
                                self.call_icing(other_team, &copy);
                            } else {
                                if let HQMIcingStatus::NotTouched(_) = other_icing_status {
                                    *other_icing_status = HQMIcingStatus::No;
                                }
                                if let HQMIcingStatus::NotTouched(_) = icing_status {
                                    *icing_status = HQMIcingStatus::No;
                                } else if icing_status.is_warning() {
                                    *icing_status = HQMIcingStatus::No;
                                    self.add_server_chat_message(String::from("Icing waved off"));
                                }
                            }
                        }

                    }
                },
                HQMSimulationEvent::PuckEnteredOtherHalf {
                    team, puck
                } => {
                    let icing_status = match team {
                        HQMTeam::Red => & mut self.game.red_icing_status,
                        HQMTeam::Blue => & mut self.game.blue_icing_status,
                    };
                    if let HQMGameObject::Puck(puck) = & self.game.world.objects[puck] {
                        if let Some(touch) = puck.touches.front() {
                            if team == touch.team && *icing_status == HQMIcingStatus::No {
                                *icing_status = HQMIcingStatus::NotTouched(touch.puck_pos.clone());
                            }
                        }
                    }
                },
                HQMSimulationEvent::PuckPassedGoalLine {
                    team, puck: _
                } => {
                    let icing_status = match team {
                        HQMTeam::Red => & mut self.game.red_icing_status,
                        HQMTeam::Blue => & mut self.game.blue_icing_status,
                    };
                    if let HQMIcingStatus::NotTouched(p) = icing_status {
                        match self.config.icing {
                            HQMIcingConfiguration::Touch => {
                                *icing_status = HQMIcingStatus::Warning(p.clone());
                                self.add_server_chat_message(String::from("Icing warning"));
                            }
                            HQMIcingConfiguration::NoTouch => {
                                let copy = p.clone();
                                self.call_icing(team, &copy);
                            }
                            HQMIcingConfiguration::Off => {}
                        }
                    }
                },
                HQMSimulationEvent::PuckEnteredOffensiveZone {
                    team, puck
                } => {
                    let offside_status = match team {
                        HQMTeam::Red => & mut self.game.red_offside_status,
                        HQMTeam::Blue => & mut self.game.blue_offside_status,
                    };
                    if let HQMGameObject::Puck(puck) = & self.game.world.objects[puck] {
                        if let Some(touch) = puck.touches.front() {
                            if team == touch.team &&
                                HQMServer::has_players_in_offensive_zone(& self.game.world, team) {
                                match self.config.offside {
                                    HQMOffsideConfiguration::Delayed => {
                                        *offside_status = HQMOffsideStatus::Warning(touch.puck_pos.clone(), touch.player_index);
                                        self.add_server_chat_message(String::from("Offside warning"));
                                    }
                                    HQMOffsideConfiguration::Immediate => {
                                        let copy = touch.puck_pos.clone();
                                        self.call_offside(team, &copy);
                                    },
                                    HQMOffsideConfiguration::Off => {}
                                }
                            }
                        }
                    }

                },
                HQMSimulationEvent::PuckLeftOffensiveZone {
                    team, puck: _
                } => {
                    let offside_status = match team {
                        HQMTeam::Red => & mut self.game.red_offside_status,
                        HQMTeam::Blue => & mut self.game.blue_offside_status,
                    };
                    if offside_status.is_warning() {
                        *offside_status = HQMOffsideStatus::No;
                        self.add_server_chat_message(String::from("Offside waved off"));
                    }
                }
            }
        }
        if self.game.red_offside_status.is_warning()
            && !HQMServer::has_players_in_offensive_zone(& self.game.world,HQMTeam::Red) {
            self.game.red_offside_status = HQMOffsideStatus::No;
            self.add_server_chat_message(String::from("Offside waved off"));
        }
        if self.game.blue_offside_status.is_warning()
            && !HQMServer::has_players_in_offensive_zone(& self.game.world,HQMTeam::Blue) {
            self.game.blue_offside_status = HQMOffsideStatus::No;
            self.add_server_chat_message(String::from("Offside waved off"));
        }
    }

    fn update_puck_touches(puck: & mut HQMPuck, player_index: usize, team: HQMTeam, time: u32) {
        let puck_pos = puck.body.pos.clone();
        let most_recent_touch = puck.touches.front_mut();
        if let Some(most_recent_touch) = most_recent_touch {
            if most_recent_touch.player_index == player_index
                && most_recent_touch.team == team {
                most_recent_touch.puck_pos = puck_pos;
                most_recent_touch.time = time;
                most_recent_touch.is_first_touch = false;
            } else {
                puck.touches.push_front(HQMPuckTouch {
                    player_index,
                    team,
                    puck_pos,
                    time,
                    is_first_touch: true
                });
            }
        } else {
            puck.touches.push_front(HQMPuckTouch {
                player_index,
                team,
                puck_pos,
                time,
                is_first_touch: true
            });
        };

        puck.touches.truncate(8);
    }

    fn get_offside_faceoff_spot(pos: &Point3<f32>, rink: & HQMRink, team: HQMTeam) -> HQMFaceoffSpot {
        let left_side = if pos.x <= rink.width/2.0 { 0usize } else { 1usize };
        let (lines_and_net, f1, f2, f3) = match team {
            HQMTeam::Red => {
                (& rink.red_lines_and_net, &rink.blue_neutral_faceoff_spots, &rink.red_neutral_faceoff_spots, &rink.red_zone_faceoff_spots)
            }
            HQMTeam::Blue => {
                (& rink.blue_lines_and_net, &rink.red_neutral_faceoff_spots, &rink.blue_neutral_faceoff_spots, &rink.blue_zone_faceoff_spots)
            }
        };
        if lines_and_net.offensive_line.point_past_middle_of_line(pos) {
            f1[left_side].clone()
        } else if lines_and_net.mid_line.point_past_middle_of_line(pos) {
            rink.center_faceoff_spot.clone()
        } else if lines_and_net.defensive_line.point_past_middle_of_line(pos) {
            f2[left_side].clone()
        } else {
            f3[left_side].clone()
        }
    }

    fn get_icing_faceoff_spot(pos: &Point3<f32>, rink: & HQMRink, team: HQMTeam) -> HQMFaceoffSpot {
        let left_side = if pos.x <= rink.width/2.0 { 0usize } else { 1usize };
        match team {
            HQMTeam::Red => {
                rink.red_zone_faceoff_spots[left_side].clone()
            }
            HQMTeam::Blue => {
                rink.blue_zone_faceoff_spots[left_side].clone()
            },
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


    async fn send_update(game: &HQMGame, player: &HQMConnectedPlayer, player_index: usize, socket: & UdpSocket, packets: &[HQMObjectPacket], write_buf: & mut [u8]) {
        let mut writer = HQMMessageWriter::new(write_buf);

        let rules_state =
            if game.red_offside_status == HQMOffsideStatus::Offside ||
                game.blue_offside_status == HQMOffsideStatus::Offside {
                HQMRulesState::Offside
            } else if game.red_icing_status == HQMIcingStatus::Icing ||
                game.blue_icing_status == HQMIcingStatus::Icing {
                HQMRulesState::Icing
            } else {
                let icing_warning = game.red_icing_status.is_warning() ||
                    game.blue_icing_status.is_warning();
                let offside_warning = game.red_offside_status.is_warning() ||
                    game.red_offside_status.is_warning();
                HQMRulesState::Regular {
                    offside_warning, icing_warning
                }
            };
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
            writer.write_bits(16, game.goal_timer);
            writer.write_bits(8, game.period);
            writer.write_bits(8, player_index as u32);

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

            writer.write_u32_aligned(game.packet);
            writer.write_u32_aligned(player.known_packet);

            for i in 0..32 {
                match &packets[i] {
                    HQMObjectPacket::Puck(puck) => {
                        writer.write_bits(1, 1);
                        writer.write_bits(2, 1); // Puck type
                        writer.write_pos(17, puck.pos.0, None);
                        writer.write_pos(17, puck.pos.1, None);
                        writer.write_pos(17, puck.pos.2, None);
                        writer.write_pos(31, puck.rot.0, None);
                        writer.write_pos(31, puck.rot.1, None);
                    } ,
                    HQMObjectPacket::Skater(skater) => {
                        writer.write_bits(1, 1);
                        writer.write_bits(2, 0); // Skater type
                        writer.write_pos(17, skater.pos.0, None);
                        writer.write_pos(17, skater.pos.1, None);
                        writer.write_pos(17, skater.pos.2, None);
                        writer.write_pos(31, skater.rot.0, None);
                        writer.write_pos(31, skater.rot.1, None);
                        writer.write_pos(13, skater.stick_pos.0, None);
                        writer.write_pos(13, skater.stick_pos.1, None);
                        writer.write_pos(13, skater.stick_pos.2, None);
                        writer.write_pos(25, skater.stick_rot.0, None);
                        writer.write_pos(25, skater.stick_rot.1, None);
                        writer.write_pos(16, skater.head_rot, None);
                        writer.write_pos(16, skater.body_rot, None);
                    },
                    HQMObjectPacket::None => {
                        writer.write_bits(1, 0);
                    }
                }
            }

            let remaining_messages = min(player.messages.len() - player.known_msgpos as usize, 15);

            writer.write_bits(4, remaining_messages as u32);
            writer.write_bits(16, player.known_msgpos.into());

            let pos2 = player.known_msgpos as usize;

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
        }

        let slice = writer.get_slice();
        let _ = socket.send_to(slice, player.addr).await;
    }

    pub(crate) fn new_game(&mut self) {
        self.game_alloc += 1;
        self.game = HQMGame::new(self.game_alloc, &self.config);

        let puck_line_start= self.game.world.rink.width / 2.0 - 0.4 * ((self.config.warmup_pucks - 1) as f32);

        for i in 0..self.config.warmup_pucks {
            let pos = Point3::new(puck_line_start + 0.8*(i as f32), 1.5, self.game.world.rink.length / 2.0);
            let rot = Matrix3::identity();
            self.game.world.create_puck_object(pos, rot, self.config.cylinder_puck_post_collision);
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

        let mut red_players: Vec<(usize, &str)> = vec![];
        let mut blue_players: Vec<(usize, &str)> = vec![];
        for (player_index, player) in players.iter().enumerate() {
            if let Some(player) = player {
                let team = player.skater.and_then(|i| match &objects[i] {
                    HQMGameObject::Player(skater) => { Some(skater.team)},
                    _ => None
                });
                if team == Some(HQMTeam::Red) {
                    red_players.push((player_index, &player.faceoff_position));
                } else if team == Some(HQMTeam::Blue) {
                    blue_players.push((player_index, &player.faceoff_position));
                }
            }
        }

        fn setup_position (positions: & mut HashMap<usize, (HQMTeam, String)>, players: &[(usize, &str)], allowed_positions: &[String], team: HQMTeam) {
            let mut available_positions = Vec::from(allowed_positions);
            for (player_index, player_position) in players.iter() {
                if let Some(x) = available_positions.iter().position(|x| *x == *player_position) {
                    let s = available_positions.remove(x);
                    positions.insert(*player_index, (team, s));
                }
            }
            let c = String::from("C");
            for (player_index, player_position) in players.iter() {
                if !positions.contains_key(player_index) {
                    if let Some(x) = available_positions.iter().position(|x| *x == c) {
                        available_positions.remove(x);
                        positions.insert(*player_index, (team, c.clone()));
                    } else if !available_positions.is_empty() {
                        let x = available_positions.remove(0);
                        positions.insert(*player_index, (team, x));
                    } else {
                        positions.insert(*player_index, (team, String::from(*player_position)));
                    }
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
        let rink = &self.game.world.rink;
        let faceoff_spot = &self.game.next_faceoff_spot;

        let positions = Self::get_faceoff_positions(& self.players, & self.game.world.objects, &rink.allowed_positions);

        let puck_pos = &faceoff_spot.center_position + &(1.5f32*Vector3::y());

        self.game.world.objects = vec![HQMGameObject::None; 32];
        self.game.world.create_puck_object(puck_pos, Matrix3::identity(), self.config.cylinder_puck_post_collision);

        let mut messages = Vec::new();

        fn setup (messages: & mut Vec<HQMMessage>, world: & mut HQMGameWorld,
                  player: & mut HQMConnectedPlayer, player_index: usize, pos: Point3<f32>, rot: Matrix3<f32>, team: HQMTeam) {
            let new_object_index = world.create_player_object(team,pos, rot, player.hand, player_index);
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
                setup (& mut messages, & mut self.game.world, player, player_index, player_position,
                       player_rotation.matrix().clone_owned(), team)
            }

        }

        self.game.red_icing_status = HQMIcingStatus::No;
        self.game.blue_icing_status = HQMIcingStatus::No;
        self.game.red_offside_status = HQMOffsideStatus::No;
        self.game.blue_offside_status = HQMOffsideStatus::No;

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

            if self.game.intermission > 0 {
                self.game.intermission -= 1;
                if self.game.intermission == 0 {
                    if self.game.game_over {
                        self.new_game();
                    } else {
                        if self.game.time == 0 {
                            self.game.time = self.config.time_period*100;
                        }
                        self.do_faceoff();
                    }

                }
            } else if self.game.goal_timer > 0 {
                self.game.goal_timer -= 1;
                if self.game.goal_timer == 0 && !self.game.game_over {
                    self.do_faceoff();
                }
            } else if self.game.time > 0 {
                self.game.time -= 1;
                if self.game.time == 0 {
                    self.game.period += 1;
                    if self.game.period > 3 && self.game.red_score != self.game.blue_score {
                        self.game.intermission = self.config.time_intermission*100;
                        self.game.game_over = true;
                    } else {
                        self.game.intermission = self.config.time_intermission*100;
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
        Ok(s) => Some(s),
        Err(_) => None
    };
}

async fn notify_master_server(socket: & UdpSocket) -> std::io::Result<usize> {
    let server_addr: SocketAddr = MASTER_SERVER.parse().unwrap();
    let msg = b"Hock\x20";
    socket.send_to(msg, server_addr).await
}

pub(crate) struct HQMConnectedPlayer {
    pub(crate) player_name: String,
    pub(crate) addr: SocketAddr,
    client_version: u8,
    pub(crate) faceoff_position: String,
    pub(crate) skater: Option<usize>,
    game_id: u32,
    input: HQMPlayerInput,
    known_packet: u32,
    known_msgpos: u16,
    chat_rep: u32,
    messages: Vec<Rc<HQMMessage>>,
    inactivity: u32,
    pub(crate) is_admin: bool,
    pub(crate) is_muted:bool,
    pub(crate) team_switch_timer: u32,
    hand: HQMSkaterHand,
    deltatime: u32
}

impl HQMConnectedPlayer {
    pub fn new(player_name: String, addr: SocketAddr, global_messages: Vec<Rc<HQMMessage>>) -> Self {
        HQMConnectedPlayer {
            player_name,
            addr,
            client_version: 0,
            faceoff_position: String::from("C"),
            skater: None,
            game_id: u32::MAX,
            known_packet: u32::MAX,
            known_msgpos: 0,
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
pub enum HQMServerMode {
    Match,
    PermanentWarmup
}

pub(crate) struct HQMServerConfiguration {
    pub(crate) server_name: String,
    pub(crate) port: u16,
    pub(crate) public: bool,
    pub(crate) player_max: u32,
    pub(crate) team_max: u32,
    pub(crate) force_team_size_parity: bool,
    pub(crate) welcome: Vec<String>,
    pub(crate) mode: HQMServerMode,

    pub(crate) password: String,

    pub(crate) time_period: u32,
    pub(crate) time_warmup: u32,
    pub(crate) time_intermission: u32,
    pub(crate) offside: HQMOffsideConfiguration,
    pub(crate) icing: HQMIcingConfiguration,
    pub(crate) warmup_pucks: u32,
    pub(crate) limit_jump_speed: bool,

    pub(crate) entry_point_red: Vector3<f32>,
    pub(crate) entry_point_blue: Vector3<f32>,
    pub(crate) entry_rotation_red: f32,
    pub(crate) entry_rotation_blue: f32,
    pub(crate) cylinder_puck_post_collision: bool
}