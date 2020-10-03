use std::net::{SocketAddr};
use nalgebra::{Vector3, Point3, Matrix3, Vector2};
use std::cmp::min;
use std::time::Duration;

mod hqm_parse;
mod hqm_simulate;

use hqm_parse::{HQMClientParser, HQMServerWriter, HQMObjectPacket};
use hqm_parse::{HQMPuckPacket, HQMSkaterPacket};
use tokio::net::UdpSocket;

const GAME_HEADER: &[u8] = b"Hock";

const MASTER_SERVER: &str = "66.226.72.227:27590";

struct HQMGame {

    objects: Vec<HQMGameObject>,
    global_messages: Vec<HQMMessage>,
    red_score: u32,
    blue_score: u32,
    period: u32,
    time: u32,
    timeout: u32,
    game_id: u32,
    game_step: u32,
    packet: u32,
    rink: HQMRink
}

struct HQMRink {
    planes: Vec<(Point3<f32>, Vector3<f32>)>,
    corners: Vec<(Point3<f32>, Vector3<f32>, f32)>
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
        HQMRink {
            planes,
            corners
        }
    }
}

impl HQMGame {
    fn new (game_id: u32) -> Self {
        let mut object_vec = Vec::with_capacity(32);
        for _ in 0..32 {
            object_vec.push(HQMGameObject::None);
        }
        for x in 0..5 {
            for y in 0..5 {
                let i = 5*x + y;
                object_vec[i as usize] = HQMGameObject::Puck(HQMPuck {
                    body: HQMBody {
                        pos: Point3::new(15.0 + ((x-2) as f32) * 2.0, 1.5, 30.5 + ((y-2) as f32) * 2.0),
                        linear_velocity: Vector3::new(0.0, 0.0, 0.0),
                        rot: Matrix3::identity(),
                        angular_velocity: Vector3::new(0.0,0.0,0.0),
                        rot_mul: Vector3::new(223.5, 128.0, 223.5)
                    },
                    radius: 0.125,
                    height: 0.0412500016391,
                });
            }
        }


        HQMGame {
            objects: object_vec,
            global_messages: vec![],
            red_score: 0,
            blue_score: 0,
            period: 0,
            time: 30000,
            timeout: 0,
            game_id,
            game_step: 0,
            packet: 0,
            rink: HQMRink::new(30.0, 61.0, 8.5)
        }
    }
}

struct HQMServer {
    players: Vec<Option<HQMConnectedPlayer>>,
    server_name: Vec<u8>,
    team_max: u32,
    game: HQMGame,
    public: bool,
    game_alloc: u32,
    addr: SocketAddr
}

impl HQMServer {
    async fn handle_message(&mut self, (size, addr): (usize, SocketAddr), socket: & mut UdpSocket, buf: &[u8]) {
        let mut parser = hqm_parse::HQMClientParser::new(&buf[0..size]);
        let header = parser.read_bytes_aligned(4);
        if header != GAME_HEADER {
            return;
        }

        let command = parser.read_byte_aligned();
        match command {
            0 => {
                self.request_info(socket, &addr, &mut parser).await;
            },
            2 => {
                self.player_join(&addr, &mut parser);
            },
            4 => {
                self.player_update(&addr, &mut parser);
            },
            7 => {
                self.player_quit(&addr);
            },
            _ => {}
        }
    }

    async fn request_info<'a>(&self, socket: & mut UdpSocket, addr: &SocketAddr, parser: &mut HQMClientParser<'a>) -> std::io::Result<usize> {
        let _player_version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();
        let mut buf = [0u8; 1024];
        let mut writer = HQMServerWriter::new(&mut buf);
        writer.write_bytes_aligned(GAME_HEADER);
        writer.write_byte_aligned(1);
        writer.write_bits(8, 55);
        writer.write_u32_aligned(ping);

        let player_count  = self.player_count();
        writer.write_bits(8, player_count);
        writer.write_bits(4, 0);
        writer.write_bits(4, self.team_max);

        writer.write_bytes_aligned_padded(32, &*self.server_name);

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

    fn player_update(&mut self, addr: &SocketAddr, parser: &mut HQMClientParser) {
        let current_slot = HQMServer::find_player_slot(self, addr);
        let (player_index, player) = match current_slot {
            Some(x) => {
                (x, self.players[x].as_mut().unwrap())
            }
            None => {
                return;
            }
        };
        player.inactivity = 0;
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

        let packet = parser.read_u32_aligned();

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

    fn process_message(&mut self, bytes: Vec<u8>, player_index: usize) {
        let msg = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return
        };

        if self.players[player_index].is_some() {
            self.add_global_chat_message(player_index as u32, msg)
        }
    }

    fn player_quit(&mut self, addr: &SocketAddr) {
        let current_slot = HQMServer::find_player_slot(self, addr);
        match current_slot {
            Some(x) => {
                let player_name = {
                    let player = self.players[x].as_ref().unwrap();
                    player.player_name.clone()
                };
                self.remove_player(x as u32);
                let msg = format!("{} quit", player_name);
                self.add_global_chat_message(u32::MAX, msg);
            }
            None => {
                println!("Player has already quit");
            }
        }
    }

    fn remove_player(&mut self, player_index: u32) {
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

                self.add_global_message(update);

                self.players[player_index as usize] = None;
            }
            None => {
                println!("Player has already quit");
            }
        }
    }

    fn add_global_chat_message(&mut self, player_index: u32, message: String) {
        if player_index == u32::MAX {
            println!("{}", &message);
        } else if let Some(player) = & self.players[player_index as usize] {
            println!("{}: {}", &player.player_name, &message);
        }
        let chat = HQMMessage::Chat {
            player_index,
            message: message.into_bytes(),
        };

        self.add_global_message(chat);
    }

    fn player_join(&mut self, addr: &SocketAddr, parser: &mut HQMClientParser) {
        let current_slot = HQMServer::find_player_slot(self, addr);
        if current_slot.is_some() {
            return; // Player has already joined
        }
        let player_version = parser.read_bits(8);
        if player_version != 55 {
            return; // Not the right version
        }
        let player_name_bytes = parser.read_bytes_aligned(32);
        let player_name = HQMServer::get_player_name(player_name_bytes);
        match player_name {
            Some(name) => {
                self.add_player(name.clone(), &addr);
                let msg = format!("{} joined", name);
                self.add_global_chat_message(u32::MAX, msg);
            }
            _ => {}
        };
    }

    fn add_player(&mut self, player_name: String, addr: &SocketAddr) -> bool {
        let player_index = HQMServer::find_empty_player_slot(self);
        match player_index {
            Some(x) => {
                let update = HQMMessage::PlayerUpdate {
                    player_name: player_name.clone().into_bytes(),
                    team: HQMTeam::Spec,
                    player_index: x as u32,
                    object_index: None,
                    in_server: true,
                };

                self.add_global_message(update);

                let new_player = HQMConnectedPlayer::new(player_name, *addr,
                                                         self.game.global_messages.clone());
                self.players[x] = Some(new_player);
                true
            }
            _ => false
        }
    }

    fn add_global_message(&mut self, message: HQMMessage) {
        self.game.global_messages.push(message.clone());
        for player in self.players.iter_mut() {
            match player {
                Some(player) => {
                    player.messages.push(message.clone());
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
                self.remove_player(i as u32);
                let msg = format!("{} timed out", player_name);
                self.add_global_chat_message(u32::MAX, msg);
            }
        }
    }

    fn create_player_object (objects: & mut Vec<HQMGameObject>, start: Point3<f32>, rot: Matrix3<f32>) -> Option<usize> {
        let object_slot = HQMServer::find_empty_object_slot(& objects);
        if let Some(i) = object_slot {
            let linear_velocity = Vector3::new (0.0, 0.0, 0.0);
            let mut collision_balls = Vec::with_capacity(6);
            collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.0, 0.0, 0.0), & start, & rot, & linear_velocity, 0.225));
            collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.25, 0.3125, 0.0), & start, & rot, & linear_velocity, 0.25));
            collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(-0.25, 0.3125, 0.0), & start, & rot, & linear_velocity, 0.25));
            collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(-0.1875, -0.1875, 0.0), & start, & rot, & linear_velocity, 0.1875));
            collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.1875, -0.1875, 0.0), & start, & rot, & linear_velocity, 0.1875));
            collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.0, 0.5, 0.0), & start, & rot, & linear_velocity, 0.1875));
            objects[i] = HQMGameObject::Player(HQMSkater {
                body: HQMBody {
                    pos: start,
                    linear_velocity,
                    rot,
                    angular_velocity: Vector3::new (0.0, 0.0, 0.0),
                    rot_mul: Vector3::new (2.75, 6.16, 2.35)
                },
                stick_pos: Point3::new(15.0, 1.5, 15.0),
                stick_velocity: Vector3::new (0.0, 0.0, 0.0),
                stick_rot: Matrix3::identity(),
                head_rot: 0.0,
                body_rot: 0.0,
                height: 0.75,
                input: HQMPlayerInput::default(),
                old_input: HQMPlayerInput::default(),
                stick_placement: Vector2::new(0.0, 0.0),
                stick_placement_delta: Vector2::new(0.0, 0.0),
                collision_balls
            })
        }
        return object_slot;
    }

    fn move_players_between_teams(&mut self) {
        let mut new_messages = Vec::new();
        for (player_index, p) in self.players.iter_mut().enumerate() {
            if let Some(player) = p {
                if player.input.join_red() || player.input.join_blue() {
                    let new_team = if player.input.join_red() {
                        HQMTeam::Red
                    } else {
                        HQMTeam::Blue
                    };
                    if player.team != new_team {
                        player.team = new_team;
                        if player.skater.is_none() {
                            let pos = Point3::new(15.0, 1.5, 15.0);
                            let rot = Matrix3::identity();

                            if let Some(i) = HQMServer::create_player_object(& mut self.game.objects, pos, rot) {
                                player.skater = Some(i);
                            }
                        }
                        new_messages.push(HQMMessage::PlayerUpdate {
                            player_name: player.player_name.clone().into_bytes(),
                            team: player.team,
                            player_index: player_index as u32,
                            object_index: player.skater.map(|x| x as u32),
                            in_server: true
                        })
                    }
                } else if player.input.spectate() && player.team != HQMTeam::Spec {
                    player.team = HQMTeam::Spec;
                    if let Some (i) = player.skater {
                        self.game.objects[i] = HQMGameObject::None;
                        player.skater = None;

                    }
                    new_messages.push(HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone().into_bytes(),
                        team: player.team,
                        player_index: player_index as u32,
                        object_index: None,
                        in_server: true
                    });
                }
            }
        }
        for m in new_messages {
            self.add_global_message(m);
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

    async fn tick(&mut self, socket: & mut UdpSocket) {
        self.remove_inactive_players ();
        let player_count2 = self.player_count();
        if player_count2 != 0 {
            self.move_players_between_teams();
            self.copy_player_input_to_object();
            self.simulate_step();

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
                    self.send_update(p, i as u32, socket, &packets).await;
                }
            }
            self.game.packet += 1;
            self.game.game_step += 1;
        }

    }

    async fn send_update(&self, player: &HQMConnectedPlayer, i: u32, socket: & mut UdpSocket, packets: &[HQMObjectPacket]) {
        let mut buf = [0u8; 2048];
        let mut writer = HQMServerWriter::new(&mut buf);
        if player.game_id != self.game.game_id {
            writer.write_bytes_aligned(GAME_HEADER);
            writer.write_byte_aligned(6);
            writer.write_u32_aligned(self.game.game_id);
        } else {
            writer.write_bytes_aligned(GAME_HEADER);
            writer.write_byte_aligned(5);
            writer.write_u32_aligned(self.game.game_id);
            writer.write_u32_aligned(self.game.game_step);
            writer.write_bits(1, 0); // TODO: Game over
            writer.write_bits(8, self.game.red_score);
            writer.write_bits(8, self.game.blue_score);
            writer.write_bits(16, self.game.time);
            writer.write_bits(16, self.game.timeout);
            writer.write_bits(8, self.game.period);
            writer.write_bits(8, i);
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
                match message {
                    HQMMessage::Chat {
                        player_index,
                        message
                    } => {
                        writer.write_bits(6, 2);
                        writer.write_bits(6, *player_index);
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
                        writer.write_bits(6, *goal_player_index);
                        writer.write_bits(6, *assist_player_index);
                    }
                    HQMMessage::PlayerUpdate {
                        player_name,
                        team,
                        player_index,
                        object_index,
                        in_server,
                    } => {
                        writer.write_bits(6, 0);
                        writer.write_bits(6, *player_index);
                        writer.write_bits(1, if *in_server { 1 } else { 0 });
                        writer.write_bits(2, team.get_num());
                        writer.write_bits(6, object_index.unwrap_or(u32::MAX));

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
        socket.send_to(slice, player.addr).await;
    }

    fn new_game(&mut self) {
        self.game_alloc += 1;
        self.game = HQMGame::new(self.game_alloc);
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
                    player_index: i as u32,
                    object_index: None,
                    in_server: true,
                };
                messages.push(update);
            }


        }
        for message in messages {
            self.add_global_message(message);
        }


    }

    pub async fn run(&mut self) -> std::io::Result<()> {
        let mut tick_timer = tokio::time::interval(Duration::from_millis(10));
        let mut public_timer = tokio::time::interval(Duration::from_secs(2));

        let mut socket = tokio::net::UdpSocket::bind(& self.addr).await?;
        let mut buf = [0u8;1024];

        loop {
            tokio::select! {
                _ = tick_timer.tick() => {
                    self.tick(& mut socket).await;
                }
                _ = public_timer.tick(), if self.public => {
                    notify_master_server(& mut socket).await;
                }
                Ok(x) = socket.recv_from(&mut buf) => {
                    self.handle_message(x, & mut socket, & buf).await;
                }
            }
        }
        Ok(())
    }

    pub fn new(name: Vec<u8>, port: u16, team_max: u32, public: bool) -> Self {
        let mut player_vec = Vec::with_capacity(64);
        for _ in 0..64 {
            player_vec.push(None);
        }

        let addr = SocketAddr::from(([0, 0, 0, 0], port));

        HQMServer {
            server_name: name,
            addr,
            players: player_vec,
            team_max,
            game: HQMGame::new(1),
            public,
            game_alloc: 1
        }
    }
}

async fn notify_master_server(socket: &mut UdpSocket) -> std::io::Result<usize> {
    let server_addr: SocketAddr = MASTER_SERVER.parse().unwrap();
    let msg = b"Hock\x20";
    socket.send_to(msg, server_addr).await
}

struct HQMConnectedPlayer {
    player_name: String,
    addr: SocketAddr,
    team: HQMTeam,
    skater: Option<usize>,
    game_id: u32,
    input: HQMPlayerInput,
    packet: u32,
    msgpos: u32,
    chat_rep: u32,
    messages: Vec<HQMMessage>,
    inactivity: u32
}

impl HQMConnectedPlayer {
    pub fn new(player_name: String, addr: SocketAddr, global_messages: Vec<HQMMessage>) -> Self {
        HQMConnectedPlayer {
            player_name,
            addr,
            team: HQMTeam::Spec,
            skater: None,
            game_id: u32::MAX,
            packet: u32::MAX,
            msgpos: 0,
            chat_rep: 0,
            messages: global_messages,
            input: HQMPlayerInput::default(),
            inactivity: 0

        }
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

enum HQMGameObject {
    None,
    Player(HQMSkater),
    Puck(HQMPuck),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum HQMTeam {
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

struct HQMBody {
    pos: Point3<f32>,                // Measured in meters
    linear_velocity: Vector3<f32>,   // Measured in meters per hundred of a second
    rot: Matrix3<f32>,               // Rotation matrix
    angular_velocity: Vector3<f32>,  // Measured in radians per hundred of a second
    rot_mul: Vector3<f32>
}

struct HQMSkater {
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
    collision_balls: Vec<HQMSkaterCollisionBall>
}

impl HQMSkater {
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
            body_rot: get_position (16, (self.head_rot + 2.0) * 8192.0)
        }
    }
}

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

struct HQMPuck {
    body: HQMBody,
    radius: f32,
    height: f32,
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
        player_index: u32,
        object_index: Option<u32>,
        in_server: bool,
    },
    Goal {
        team: HQMTeam,
        goal_player_index: u32,
        assist_player_index: u32,
    },
    Chat {
        player_index: u32,
        message: Vec<u8>,
    },
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    return HQMServer::new(Vec::from("MigoTest".as_bytes()), 27585, 5, true).run().await;
}

