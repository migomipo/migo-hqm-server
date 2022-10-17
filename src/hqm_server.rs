use std::borrow::Cow;
use std::cmp::min;
use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::f32::consts::{FRAC_PI_2, PI};
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use nalgebra::{Point3, Rotation3, Vector2};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;
use tokio::time::MissedTickBehavior;
use tracing::info;
use uuid::Uuid;

use crate::hqm_game::{
    HQMGame, HQMGameObject, HQMObjectIndex, HQMPlayerInput, HQMRink, HQMRulesState, HQMSkater,
    HQMSkaterHand, HQMTeam,
};
use crate::hqm_parse::{HQMMessageReader, HQMMessageWriter};
use crate::hqm_simulate::HQMSimulationEvent;

const GAME_HEADER: &[u8] = b"Hock";

struct HQMServerReceivedData {
    addr: SocketAddr,
    data: Bytes,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum HQMClientVersion {
    Vanilla,
    Ping,
    PingRules,
}

impl HQMClientVersion {
    fn has_ping(self) -> bool {
        match self {
            HQMClientVersion::Vanilla => false,
            HQMClientVersion::Ping => true,
            HQMClientVersion::PingRules => true,
        }
    }

    fn has_rules(self) -> bool {
        match self {
            HQMClientVersion::Vanilla => false,
            HQMClientVersion::Ping => false,
            HQMClientVersion::PingRules => true,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct HQMServerPlayerIndex(pub usize);

impl std::fmt::Display for HQMServerPlayerIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for HQMServerPlayerIndex {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(HQMServerPlayerIndex)
    }
}

pub struct HQMServerPlayerList {
    players: Vec<Option<HQMServerPlayer>>,
}

impl HQMServerPlayerList {
    pub fn len(&self) -> usize {
        self.players.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (HQMServerPlayerIndex, Option<&HQMServerPlayer>)> {
        self.players
            .iter()
            .enumerate()
            .map(|(i, p)| (HQMServerPlayerIndex(i), p.as_ref()))
    }

    fn iter_mut(
        &mut self,
    ) -> impl Iterator<Item = (HQMServerPlayerIndex, Option<&mut HQMServerPlayer>)> {
        self.players
            .iter_mut()
            .enumerate()
            .map(|(i, p)| (HQMServerPlayerIndex(i), p.as_mut()))
    }

    pub fn get(
        &self,
        HQMServerPlayerIndex(player_index): HQMServerPlayerIndex,
    ) -> Option<&HQMServerPlayer> {
        if let Some(x) = self.players.get(player_index) {
            x.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn get_mut(
        &mut self,
        HQMServerPlayerIndex(player_index): HQMServerPlayerIndex,
    ) -> Option<&mut HQMServerPlayer> {
        if let Some(x) = self.players.get_mut(player_index) {
            x.as_mut()
        } else {
            None
        }
    }

    pub fn get_from_object_index(
        &mut self,
        object_index: HQMObjectIndex,
    ) -> Option<(HQMServerPlayerIndex, HQMTeam, &HQMServerPlayer)> {
        for (player_index, player) in self.players.iter().enumerate() {
            if let Some(player) = player {
                if let Some((o, team)) = player.object {
                    if o == object_index {
                        return Some((HQMServerPlayerIndex(player_index), team, player));
                    }
                }
            }
        }
        None
    }

    fn remove_player(&mut self, HQMServerPlayerIndex(player_index): HQMServerPlayerIndex) {
        self.players[player_index] = None;
    }

    fn add_player(
        &mut self,
        HQMServerPlayerIndex(player_index): HQMServerPlayerIndex,
        player: HQMServerPlayer,
    ) {
        self.players[player_index] = Some(player);
    }
}

enum HQMWaitingMessageReceiver {
    All,
    Specific(HQMServerPlayerIndex),
}

#[derive(Debug, Clone)]
pub enum HQMMessage {
    PlayerUpdate {
        player_name: Rc<String>,
        object: Option<(HQMObjectIndex, HQMTeam)>,
        player_index: HQMServerPlayerIndex,
        in_server: bool,
    },
    Goal {
        team: HQMTeam,
        goal_player_index: Option<HQMServerPlayerIndex>,
        assist_player_index: Option<HQMServerPlayerIndex>,
    },
    Chat {
        player_index: Option<HQMServerPlayerIndex>,
        message: Cow<'static, str>,
    },
}

pub struct HQMServerMessages {
    persistent_messages: Vec<Rc<HQMMessage>>,
    replay_messages: Vec<Rc<HQMMessage>>,
    waiting_messages: VecDeque<(HQMWaitingMessageReceiver, Rc<HQMMessage>)>,
}

impl HQMServerMessages {
    fn new() -> Self {
        Self {
            persistent_messages: Vec::with_capacity(1024),
            replay_messages: Vec::with_capacity(1024),
            waiting_messages: VecDeque::with_capacity(64),
        }
    }

    fn clear(&mut self) {
        self.persistent_messages.clear();
        self.replay_messages.clear();
        self.waiting_messages.clear();
    }

    fn get_persistent_messages(&self) -> &[Rc<HQMMessage>] {
        self.persistent_messages.as_slice()
    }

    fn get_replay_messages(&self) -> &[Rc<HQMMessage>] {
        self.replay_messages.as_slice()
    }

    pub fn add_user_chat_message(&mut self, message: String, sender_index: HQMServerPlayerIndex) {
        let chat = HQMMessage::Chat {
            player_index: Some(sender_index),
            message: Cow::Owned(message),
        };
        self.add_global_message(chat, false, true);
    }

    pub fn add_server_chat_message(&mut self, message: String) {
        let chat = HQMMessage::Chat {
            player_index: None,
            message: Cow::Owned(message),
        };
        self.add_global_message(chat, false, true);
    }

    pub fn add_server_chat_message_str(&mut self, message: &'static str) {
        let chat = HQMMessage::Chat {
            player_index: None,
            message: Cow::Borrowed(message),
        };
        self.add_global_message(chat, false, true);
    }

    pub fn add_directed_chat_message(
        &mut self,
        message: String,
        receiver_index: HQMServerPlayerIndex,
        sender_index: Option<HQMServerPlayerIndex>,
    ) {
        let chat = HQMMessage::Chat {
            player_index: sender_index,
            message: Cow::Owned(message),
        };
        self.add_directed_message(chat, receiver_index);
    }

    pub fn add_directed_chat_message_str(
        &mut self,
        message: &'static str,
        receiver_index: HQMServerPlayerIndex,
        sender_index: Option<HQMServerPlayerIndex>,
    ) {
        let chat = HQMMessage::Chat {
            player_index: sender_index,
            message: Cow::Borrowed(message),
        };
        self.add_directed_message(chat, receiver_index);
    }

    pub fn add_directed_user_chat_message(
        &mut self,
        message: String,
        receiver_index: HQMServerPlayerIndex,
        sender_index: HQMServerPlayerIndex,
    ) {
        self.add_directed_chat_message(message, receiver_index, Some(sender_index));
    }

    pub fn add_directed_server_chat_message(
        &mut self,
        message: String,
        receiver_index: HQMServerPlayerIndex,
    ) {
        self.add_directed_chat_message(message, receiver_index, None);
    }

    pub fn add_directed_server_chat_message_str(
        &mut self,
        message: &'static str,
        receiver_index: HQMServerPlayerIndex,
    ) {
        self.add_directed_chat_message_str(message, receiver_index, None);
    }

    pub fn add_goal_message(
        &mut self,
        team: HQMTeam,
        goal_player_index: Option<HQMServerPlayerIndex>,
        assist_player_index: Option<HQMServerPlayerIndex>,
    ) {
        let message = HQMMessage::Goal {
            team,
            goal_player_index,
            assist_player_index,
        };
        self.add_global_message(message, true, true);
    }

    fn add_global_message(&mut self, message: HQMMessage, persistent: bool, replay: bool) {
        let rc = Rc::new(message);
        if replay {
            self.replay_messages.push(rc.clone());
        }
        if persistent {
            self.persistent_messages.push(rc.clone());
        }
        self.waiting_messages
            .push_front((HQMWaitingMessageReceiver::All, rc));
    }

    fn add_directed_message(&mut self, message: HQMMessage, receiver: HQMServerPlayerIndex) {
        let rc = Rc::new(message);
        self.waiting_messages
            .push_back((HQMWaitingMessageReceiver::Specific(receiver), rc));
    }
}

pub struct HQMServer {
    pub players: HQMServerPlayerList,
    pub messages: HQMServerMessages,
    pub(crate) ban_list: HashSet<std::net::IpAddr>,
    pub(crate) allow_join: bool,
    pub config: HQMServerConfiguration,
    pub game: HQMGame,
    replay_queue: VecDeque<ReplayElement>,
    game_id: u32,
    pub is_muted: bool,
}

impl HQMServer {
    async fn handle_message<B: HQMServerBehaviour>(
        &mut self,
        addr: SocketAddr,
        socket: &Arc<UdpSocket>,
        msg: &[u8],
        behaviour: &mut B,
        write_buf: &mut [u8],
    ) {
        let mut parser = HQMMessageReader::new(&msg);
        let header = parser.read_bytes_aligned(4);
        if header != GAME_HEADER {
            return;
        }

        let command = parser.read_byte_aligned();
        match command {
            0 => {
                self.request_info(socket, addr, &mut parser, behaviour, write_buf)
                    .await;
            }
            2 => {
                self.player_join(addr, &mut parser, behaviour);
            }
            4 => {
                self.player_update(addr, &mut parser, HQMClientVersion::Vanilla, behaviour);
            }
            8 => {
                self.player_update(addr, &mut parser, HQMClientVersion::Ping, behaviour);
            }
            0x10 => {
                self.player_update(addr, &mut parser, HQMClientVersion::PingRules, behaviour);
            }
            7 => {
                self.player_exit(addr, behaviour);
            }
            _ => {}
        }
    }

    async fn request_info<'a, B: HQMServerBehaviour>(
        &self,
        socket: &Arc<UdpSocket>,
        addr: SocketAddr,
        parser: &mut HQMMessageReader<'a>,
        behaviour: &B,
        write_buf: &mut [u8],
    ) {
        let _player_version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();

        let mut writer = HQMMessageWriter::new(write_buf);
        writer.write_bytes_aligned(GAME_HEADER);
        writer.write_byte_aligned(1);
        writer.write_bits(8, 55);
        writer.write_u32_aligned(ping);

        let player_count = self.player_count();
        writer.write_bits(8, player_count as u32);
        writer.write_bits(4, 4);
        writer.write_bits(4, behaviour.get_number_of_players() as u32);

        writer.write_bytes_aligned_padded(32, self.config.server_name.as_ref());

        let written = writer.get_bytes_written();
        let socket = socket.clone();
        let addr = addr.clone();

        let slice = &write_buf[0..written];
        let _ = socket.send_to(slice, addr).await;
    }

    fn player_count(&self) -> usize {
        let mut player_count = 0;
        for (_, player) in self.players.iter() {
            if let Some(player) = player {
                let is_actual_player = match player.data {
                    HQMServerPlayerData::NetworkPlayer { .. } => true,
                    HQMServerPlayerData::DualControl { .. } => false,
                };
                if is_actual_player {
                    player_count += 1;
                }
            }
        }
        player_count
    }

    fn player_update<B: HQMServerBehaviour>(
        &mut self,
        addr: SocketAddr,
        parser: &mut HQMMessageReader,
        client_version: HQMClientVersion,
        behaviour: &mut B,
    ) {
        let current_slot = self.find_player_slot(addr);
        let (player_index, player) = match current_slot {
            Some(x) => (x, self.players.get_mut(x).unwrap()),
            None => {
                return;
            }
        };
        if let HQMServerPlayerData::NetworkPlayer { data } = &mut player.data {
            let current_game_id = parser.read_u32_aligned();

            let input_stick_angle = parser.read_f32_aligned();
            let input_turn = parser.read_f32_aligned();
            let _input_unknown = parser.read_f32_aligned();
            let input_fwbw = parser.read_f32_aligned();
            let input_stick_rot_1 = parser.read_f32_aligned();
            let input_stick_rot_2 = parser.read_f32_aligned();
            let input_head_rot = parser.read_f32_aligned();
            let input_body_rot = parser.read_f32_aligned();
            let input_keys = parser.read_u32_aligned();
            let input = HQMPlayerInput {
                stick_angle: input_stick_angle,
                turn: input_turn,
                fwbw: input_fwbw,
                stick: Vector2::new(input_stick_rot_1, input_stick_rot_2),
                head_rot: input_head_rot,
                body_rot: input_body_rot,
                keys: input_keys,
            };

            let deltatime = if client_version.has_ping() {
                Some(parser.read_u32_aligned())
            } else {
                None
            };

            let new_known_packet = parser.read_u32_aligned();
            let known_msgpos = parser.read_u16_aligned() as usize;

            let time_received = Instant::now();

            let chat = {
                let has_chat_msg = parser.read_bits(1) == 1;
                if has_chat_msg {
                    let rep = parser.read_bits(3) as u8;
                    let byte_num = parser.read_bits(8) as usize;
                    let message = parser.read_bytes_aligned(byte_num);
                    Some((rep, message))
                } else {
                    None
                }
            };

            let duration_since_packet =
                if data.game_id == current_game_id && data.known_packet < new_known_packet {
                    let ticks = &self.game.saved_pings;
                    self.game
                        .packet
                        .checked_sub(new_known_packet)
                        .and_then(|diff| ticks.get(diff as usize))
                        .and_then(|last_time_received| {
                            time_received.checked_duration_since(*last_time_received)
                        })
                } else {
                    None
                };

            if let Some(duration_since_packet) = duration_since_packet {
                data.last_ping.truncate(100 - 1);
                data.last_ping
                    .push_front(duration_since_packet.as_secs_f32());
            }

            data.inactivity = 0;
            data.client_version = client_version;
            data.known_packet = new_known_packet;
            player.input = input;
            data.game_id = current_game_id;
            data.known_msgpos = known_msgpos;

            if let Some(deltatime) = deltatime {
                data.deltatime = deltatime;
            }

            if let Some((rep, message)) = chat {
                if data.chat_rep != Some(rep) {
                    data.chat_rep = Some(rep);
                    self.process_message(message, player_index, behaviour);
                }
            }
        }
    }

    fn player_join<B: HQMServerBehaviour>(
        &mut self,
        addr: SocketAddr,
        parser: &mut HQMMessageReader,
        behaviour: &mut B,
    ) {
        let player_count = self.player_count();
        let max_player_count = self.config.player_max;
        if player_count >= max_player_count {
            return; // Ignore join request
        }
        let player_version = parser.read_bits(8);
        if player_version != 55 {
            return; // Not the right version
        }
        let current_slot = self.find_player_slot(addr);
        if current_slot.is_some() {
            return; // Player has already joined
        }

        // Check ban list
        if self.ban_list.contains(&addr.ip()) {
            return;
        }

        // Disabled join
        if !self.allow_join {
            return;
        }

        let player_name_bytes = parser.read_bytes_aligned(32);
        let player_name = get_player_name(player_name_bytes);
        match player_name {
            Some(name) => {
                if let Some(player_index) = self.add_player(name.clone(), addr) {
                    behaviour.after_player_join(self, player_index);
                    info!(
                        "{} ({}) joined server from address {:?}",
                        name, player_index, addr
                    );
                    let msg = format!("{} joined", name);
                    self.messages.add_server_chat_message(msg);
                }
            }
            _ => {}
        };
    }

    pub fn set_hand(&mut self, hand: HQMSkaterHand, player_index: HQMServerPlayerIndex) {
        if let Some(player) = self.players.get_mut(player_index) {
            player.hand = hand;
            let object_index = player.object.map(|x| x.0);

            fn change_skater(
                server: &mut HQMServer,
                object_index: HQMObjectIndex,
                msg_player_index: HQMServerPlayerIndex,
                hand: HQMSkaterHand,
            ) {
                if let Some(skater) = server.game.world.objects.get_skater_mut(object_index) {
                    if server.game.period != 0 {
                        server.messages.add_directed_server_chat_message_str(
                            "Stick hand will change after next intermission",
                            msg_player_index,
                        );

                        return;
                    }

                    skater.hand = hand;
                }
            }

            if let Some((dual_control_index, _, stick)) = self.get_dual_control_player(player_index)
            {
                if stick == Some(player_index) {
                    if let Some(dual_control_player) = self.players.get_mut(dual_control_index) {
                        dual_control_player.hand = hand;
                        if let Some((object_index, _)) = dual_control_player.object {
                            change_skater(self, object_index, player_index, hand);
                        }
                    }
                }
            } else {
                if let Some(object_index) = object_index {
                    change_skater(self, object_index, player_index, hand);
                }
            };
        }
    }

    fn process_command<B: HQMServerBehaviour>(
        &mut self,
        command: &str,
        arg: &str,
        player_index: HQMServerPlayerIndex,
        behaviour: &mut B,
    ) {
        match command {
            "enablejoin" => {
                self.set_allow_join(player_index, true);
            }
            "disablejoin" => {
                self.set_allow_join(player_index, false);
            }
            "mute" => {
                if let Ok(mute_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.mute_player(player_index, mute_player_index);
                }
            }
            "unmute" => {
                if let Ok(mute_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.unmute_player(player_index, mute_player_index);
                }
            }
            /*"shadowmute" => {
                if let Ok(mute_player_index) = arg.parse::<usize>() {
                    if mute_player_index < self.players.len() {
                        self.shadowmute_player(player_index, mute_player_index);
                    }
                }
            },*/
            "mutechat" => {
                self.mute_chat(player_index);
            }
            "unmutechat" => {
                self.unmute_chat(player_index);
            }
            "kick" => {
                if let Ok(kick_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.kick_player(player_index, kick_player_index, false, behaviour);
                }
            }
            "kickall" => {
                self.kick_all_matching(player_index, arg, false, behaviour);
            }
            "ban" => {
                if let Ok(kick_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.kick_player(player_index, kick_player_index, true, behaviour);
                }
            }
            "banall" => {
                self.kick_all_matching(player_index, arg, true, behaviour);
            }
            "clearbans" => {
                self.clear_bans(player_index);
            }
            "lefty" => {
                self.set_hand(HQMSkaterHand::Left, player_index);
            }
            "righty" => {
                self.set_hand(HQMSkaterHand::Right, player_index);
            }
            "admin" => {
                self.admin_login(player_index, arg);
            }
            "list" => {
                if arg.is_empty() {
                    self.list_players(player_index, 0);
                } else if let Ok(first_index) = arg.parse::<usize>() {
                    self.list_players(player_index, first_index);
                }
            }
            "search" => {
                self.search_players(player_index, arg);
            }
            "ping" => {
                if let Ok(ping_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.ping(ping_player_index, player_index);
                }
            }
            "pings" => {
                if let Some((ping_player_index, _name)) = self.player_exact_unique_match(arg) {
                    self.ping(ping_player_index, player_index);
                } else {
                    let matches = self.player_search(arg);
                    if matches.is_empty() {
                        self.messages
                            .add_directed_server_chat_message_str("No matches found", player_index);
                    } else if matches.len() > 1 {
                        self.messages.add_directed_server_chat_message_str(
                            "Multiple matches found, use /ping X",
                            player_index,
                        );
                        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
                            let msg = format!("{}: {}", found_player_index, found_player_name);
                            self.messages
                                .add_directed_server_chat_message(msg, player_index);
                        }
                    } else {
                        self.ping(matches[0].0, player_index);
                    }
                }
            }
            "view" => {
                if let Ok(view_player_index) = arg.parse::<HQMServerPlayerIndex>() {
                    self.view(view_player_index, player_index);
                }
            }
            "views" => {
                if let Some((view_player_index, _name)) = self.player_exact_unique_match(arg) {
                    self.view(view_player_index, player_index);
                } else {
                    let matches = self.player_search(arg);
                    if matches.is_empty() {
                        self.messages
                            .add_directed_server_chat_message_str("No matches found", player_index);
                    } else if matches.len() > 1 {
                        self.messages.add_directed_server_chat_message_str(
                            "Multiple matches found, use /view X",
                            player_index,
                        );
                        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
                            let str = format!("{}: {}", found_player_index, found_player_name);
                            self.messages
                                .add_directed_server_chat_message(str, player_index);
                        }
                    } else {
                        self.view(matches[0].0, player_index);
                    }
                }
            }
            "restoreview" => {
                if let Some(player) = self.players.get_mut(player_index) {
                    if let HQMServerPlayerData::NetworkPlayer { data } = &mut player.data {
                        if data.view_player_index != player_index {
                            data.view_player_index = player_index;
                            self.messages.add_directed_server_chat_message_str(
                                "View has been restored",
                                player_index,
                            );
                        }
                    }
                }
            }
            "swap" => {
                self.swap_dual_control(behaviour, player_index);
            }
            "t" => {
                self.add_user_team_message(arg, player_index);
            }
            _ => behaviour.handle_command(self, command, arg, player_index),
        }
    }

    fn swap_dual_control<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some((dual_control_player_index, movement, stick)) =
            self.get_dual_control_player(player_index)
        {
            self.update_dual_control_internal(
                behaviour,
                dual_control_player_index,
                stick,
                movement,
            );
        }
    }

    fn list_players(&mut self, receiver_index: HQMServerPlayerIndex, first_index: usize) {
        let mut found = 0;

        for player_index in first_index..self.players.len() {
            let player_index = HQMServerPlayerIndex(player_index);
            if let Some(player) = self.players.get(player_index) {
                let msg = format!("{}: {}", player_index, player.player_name);
                self.messages
                    .add_directed_server_chat_message(msg, receiver_index);
                found += 1;
                if found >= 5 {
                    break;
                }
            }
        }
    }

    fn search_players(&mut self, player_index: HQMServerPlayerIndex, name: &str) {
        let matches = self.player_search(name);
        if matches.is_empty() {
            self.messages
                .add_directed_server_chat_message_str("No matches found", player_index);
            return;
        }
        for (found_player_index, found_player_name) in matches.into_iter().take(5) {
            let msg = format!("{}: {}", found_player_index, found_player_name);
            self.messages
                .add_directed_server_chat_message(msg, player_index);
        }
    }

    pub(crate) fn view(
        &mut self,
        view_player_index: HQMServerPlayerIndex,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(view_player) = self.players.get(view_player_index) {
            let view_player_name = view_player.player_name.clone();
            let has_dual_control_player = self.get_dual_control_player(player_index).is_some();
            if let Some(player) = self.players.get_mut(player_index) {
                if let HQMServerPlayerData::NetworkPlayer { data } = &mut player.data {
                    if player.object.is_some() || has_dual_control_player {
                        self.messages.add_directed_server_chat_message_str(
                            "You must be a spectator to change view",
                            player_index,
                        );
                    } else if view_player_index != data.view_player_index {
                        data.view_player_index = view_player_index;
                        if player_index != view_player_index {
                            let msg = format!("You are now viewing {}", view_player_name);
                            self.messages
                                .add_directed_server_chat_message(msg, player_index);
                        } else {
                            self.messages.add_directed_server_chat_message_str(
                                "View has been restored",
                                player_index,
                            );
                        }
                    }
                }
            }
        } else {
            self.messages.add_directed_server_chat_message_str(
                "No player with this ID exists",
                player_index,
            );
        }
    }

    fn ping(
        &mut self,
        ping_player_index: HQMServerPlayerIndex,
        player_index: HQMServerPlayerIndex,
    ) {
        if let Some(ping_player) = self.players.get(ping_player_index) {
            if let Some(ping) = ping_player.ping_data() {
                let msg1 = format!(
                    "{} ping: avg {:.0} ms",
                    ping_player.player_name,
                    (ping.avg * 1000f32)
                );
                let msg2 = format!(
                    "min {:.0} ms, max {:.0} ms, std.dev {:.1}",
                    (ping.min * 1000f32),
                    (ping.max * 1000f32),
                    (ping.deviation * 1000f32)
                );
                self.messages
                    .add_directed_server_chat_message(msg1, player_index);
                self.messages
                    .add_directed_server_chat_message(msg2, player_index);
            } else {
                self.messages.add_directed_server_chat_message_str(
                    "This player is not a connected player",
                    player_index,
                );
            }
        } else {
            self.messages.add_directed_server_chat_message_str(
                "No player with this ID exists",
                player_index,
            );
        }
    }

    pub fn player_exact_unique_match(
        &self,
        name: &str,
    ) -> Option<(HQMServerPlayerIndex, Rc<String>)> {
        let mut found = None;
        for (player_index, player) in self.players.iter() {
            if let Some(player) = player {
                if player.player_name.as_str() == name {
                    if found.is_none() {
                        found = Some((player_index, player.player_name.clone()));
                    } else {
                        return None;
                    }
                }
            }
        }
        found
    }

    pub fn player_search(
        &self,
        name: &str,
    ) -> smallvec::SmallVec<[(HQMServerPlayerIndex, Rc<String>); 64]> {
        let name = name.to_lowercase();
        let mut found = smallvec::SmallVec::<[_; 64]>::new();
        for (player_index, player) in self.players.iter() {
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

    fn process_message<B: HQMServerBehaviour>(
        &mut self,
        bytes: Vec<u8>,
        player_index: HQMServerPlayerIndex,
        behaviour: &mut B,
    ) {
        let msg = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return,
        };

        if self.players.get(player_index).is_some() {
            if msg.starts_with("/") {
                let split: Vec<&str> = msg.splitn(2, " ").collect();
                let command = &split[0][1..];
                let arg = if split.len() < 2 { "" } else { &split[1] };
                self.process_command(command, arg, player_index, behaviour);
            } else {
                if !self.is_muted {
                    match self.players.get(player_index) {
                        Some(player) => match player.is_muted {
                            HQMMuteStatus::NotMuted => {
                                info!("{} ({}): {}", &player.player_name, player_index, &msg);
                                self.messages.add_user_chat_message(msg, player_index);
                            }
                            HQMMuteStatus::ShadowMuted => {
                                self.messages.add_directed_user_chat_message(
                                    msg,
                                    player_index,
                                    player_index,
                                );
                            }
                            HQMMuteStatus::Muted => {}
                        },
                        _ => {
                            return;
                        }
                    }
                }
            }
        }
    }

    fn player_exit<B: HQMServerBehaviour>(&mut self, addr: SocketAddr, behaviour: &mut B) {
        let player_index = self.find_player_slot(addr);

        if let Some(player_index) = player_index {
            let player_name = {
                let player = self.players.get(player_index).unwrap();
                player.player_name.clone()
            };
            self.remove_player(behaviour, player_index, true);
            info!("{} ({}) exited server", player_name, player_index);
            let msg = format!("{} exited", player_name);
            self.messages.add_server_chat_message(msg);
        }
    }

    fn add_player(
        &mut self,
        player_name: String,
        addr: SocketAddr,
    ) -> Option<HQMServerPlayerIndex> {
        let player_index = self.find_empty_player_slot();
        match player_index {
            Some(player_index) => {
                let new_player = HQMServerPlayer::new_network_player(
                    player_index,
                    player_name,
                    addr,
                    self.messages.get_persistent_messages(),
                );
                let update = new_player.get_update_message(player_index);

                self.players.add_player(player_index, new_player);

                self.messages.add_global_message(update, true, true);

                let welcome = self.config.welcome.clone();
                for welcome_msg in welcome {
                    self.messages
                        .add_directed_server_chat_message(welcome_msg, player_index);
                }

                Some(player_index)
            }
            _ => None,
        }
    }

    pub fn remove_player<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        player_index: HQMServerPlayerIndex,
        on_replay: bool,
    ) {
        behaviour.before_player_exit(self, player_index);
        if let Some(player) = self.players.get(player_index) {
            let player_name = player.player_name.clone();
            let is_admin = player.is_admin;

            if let Some((object_index, _)) = player.object {
                self.game.world.remove_player(object_index);
            }

            match &player.data {
                HQMServerPlayerData::NetworkPlayer { .. } => {
                    self.remove_player_from_dual_control(behaviour, player_index);
                }
                HQMServerPlayerData::DualControl { movement, stick } => {
                    let movement = *movement;
                    let stick = *stick;
                    if let Some(movement) = movement {
                        set_view_player_index(movement, &mut self.players, movement)
                    }
                    if let Some(stick) = stick {
                        set_view_player_index(stick, &mut self.players, stick)
                    }
                }
            }

            let update = HQMMessage::PlayerUpdate {
                player_name,
                object: None,
                player_index,
                in_server: false,
            };

            self.messages.add_global_message(update, true, on_replay);

            self.players.remove_player(player_index);

            if is_admin {
                let admin_found = self
                    .players
                    .iter()
                    .any(|(_, x)| x.map_or(false, |x| x.is_admin));

                if !admin_found {
                    self.allow_join = true;
                }
            }
        }
    }

    pub fn remove_player_from_dual_control<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        player_index: HQMServerPlayerIndex,
    ) {
        let mut changes = smallvec::SmallVec::<[_; 4]>::new();
        for (i, player) in self.players.iter() {
            if let Some(player) = player {
                if let HQMServerPlayerData::DualControl { movement, stick } = &player.data {
                    let new_movement = if *movement == Some(player_index) {
                        None
                    } else {
                        *movement
                    };
                    let new_stick = if *stick == Some(player_index) {
                        None
                    } else {
                        *stick
                    };
                    if new_movement != *movement || new_stick != *stick {
                        changes.push((i, new_movement, new_stick));
                    }
                }
            }
        }
        for (i, movement, stick) in changes {
            self.update_dual_control_internal(behaviour, i, movement, stick)
        }
    }

    pub fn move_to_spectator<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        player_index: HQMServerPlayerIndex,
    ) -> bool {
        if let Some(player) = self.players.get_mut(player_index) {
            if let HQMServerPlayerData::DualControl { .. } = player.data {
                self.remove_player(behaviour, player_index, true);
                return true;
            } else {
                if let Some((object_index, _)) = player.object {
                    if self.game.world.remove_player(object_index) {
                        player.object = None;
                        let update = player.get_update_message(player_index);
                        self.messages.add_global_message(update, true, true);

                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn spawn_skater_at_spawnpoint<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        player_index: HQMServerPlayerIndex,
        team: HQMTeam,
        spawn_point: HQMSpawnPoint,
    ) -> Option<HQMObjectIndex> {
        let (pos, rot) = get_spawnpoint(&self.game.world.rink, team, spawn_point);
        self.spawn_skater(behaviour, player_index, team, pos, rot)
    }

    pub fn spawn_dual_control_skater_at_spawnpoint<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        team: HQMTeam,
        spawn_point: HQMSpawnPoint,
        movement: Option<HQMServerPlayerIndex>,
        stick: Option<HQMServerPlayerIndex>,
    ) -> Option<(HQMServerPlayerIndex, HQMObjectIndex)> {
        let (pos, rot) = get_spawnpoint(&self.game.world.rink, team, spawn_point);
        self.spawn_dual_control_skater(behaviour, team, pos, rot, movement, stick)
    }

    pub fn spawn_dual_control_skater<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        team: HQMTeam,
        pos: Point3<f32>,
        rot: Rotation3<f32>,
        movement: Option<HQMServerPlayerIndex>,
        stick: Option<HQMServerPlayerIndex>,
    ) -> Option<(HQMServerPlayerIndex, HQMObjectIndex)> {
        if movement.is_none() && stick.is_none() {
            return None;
        }

        let player_index = self.find_empty_player_slot();
        match player_index {
            Some(player_index) => {
                if let Some(skater) =
                    self.game
                        .world
                        .create_player_object(pos, rot, HQMSkaterHand::Right, 1.0)
                {
                    let new_player = HQMServerPlayer {
                        player_name: Rc::new("?/?".to_owned()),
                        object: Some((skater, team)),
                        id: Uuid::new_v4(),
                        data: HQMServerPlayerData::DualControl {
                            movement: None,
                            stick: None,
                        },
                        is_admin: false,
                        is_muted: HQMMuteStatus::NotMuted,
                        hand: HQMSkaterHand::Right,
                        mass: 1.0,
                        input: Default::default(),
                    };
                    self.players.add_player(player_index, new_player);

                    self.update_dual_control_internal(behaviour, player_index, movement, stick);

                    Some((player_index, skater))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn spawn_skater<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        player_index: HQMServerPlayerIndex,
        team: HQMTeam,
        pos: Point3<f32>,
        rot: Rotation3<f32>,
    ) -> Option<HQMObjectIndex> {
        if let Some(player) = self.players.get_mut(player_index) {
            if let Some((object_index, _)) = player.object {
                if let Some(skater) = self.game.world.objects.get_skater_mut(object_index) {
                    *skater = HQMSkater::new(pos, rot, player.hand, player.mass);
                    let object = Some((object_index, team));
                    player.object = object;
                    let update = player.get_update_message(player_index);
                    self.messages.add_global_message(update, true, true);
                }
            } else {
                if let Some(skater) =
                    self.game
                        .world
                        .create_player_object(pos, rot, player.hand, player.mass)
                {
                    if let HQMServerPlayerData::NetworkPlayer { data } = &mut player.data {
                        data.view_player_index = player_index;
                    }

                    let object = Some((skater, team));
                    player.object = object;
                    let update = player.get_update_message(player_index);
                    self.messages.add_global_message(update, true, true);
                    self.remove_player_from_dual_control(behaviour, player_index);
                    return Some(skater);
                }
            }
        }
        None
    }

    pub fn update_dual_control<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        dual_control_player_index: HQMServerPlayerIndex,
        movement: Option<HQMServerPlayerIndex>,
        stick: Option<HQMServerPlayerIndex>,
    ) {
        if movement.is_some() || stick.is_some() {
            let mut changes = smallvec::SmallVec::<[_; 4]>::new();
            for (player_index, player) in self.players.iter() {
                if player_index == dual_control_player_index {
                    continue;
                }
                if let Some(player) = player {
                    if let HQMServerPlayerData::DualControl {
                        movement: m,
                        stick: s,
                    } = player.data
                    {
                        let mut changed = false;
                        let mut new_movement = m;
                        let mut new_stick = s;
                        if m.is_some() && (m == movement || m == stick) {
                            new_movement = None;
                            changed = true;
                        }
                        if s.is_some() && (s == movement || s == stick) {
                            new_stick = None;
                            changed = true;
                        }
                        if changed {
                            changes.push((player_index, new_movement, new_stick));
                        }
                    }
                }
            }
            for (i, new_movement, new_stick) in changes {
                self.update_dual_control_internal(behaviour, i, new_movement, new_stick);
            }
        }

        self.update_dual_control_internal(behaviour, dual_control_player_index, movement, stick);
    }

    fn update_dual_control_internal<B: HQMServerBehaviour>(
        &mut self,
        behaviour: &mut B,
        dual_control_player_index: HQMServerPlayerIndex,
        movement: Option<HQMServerPlayerIndex>,
        stick: Option<HQMServerPlayerIndex>,
    ) {
        let player_name = Rc::new(get_dual_control_name(&self.players, movement, stick));
        let hand = stick
            .and_then(|x| self.players.get(x))
            .map(|player| player.hand);

        let player = self.players.get_mut(dual_control_player_index);

        if let Some(player) = player {
            if let HQMServerPlayerData::DualControl {
                movement: m,
                stick: s,
            } = &mut player.data
            {
                let old_movement = *m;
                let old_stick = *s;

                if movement.is_none() && stick.is_none() {
                    self.remove_player(behaviour, dual_control_player_index, true);
                } else {
                    *m = movement;
                    *s = stick;
                    player.player_name = player_name.clone();
                    player.id = Uuid::new_v4();
                    if let Some(hand) = hand {
                        player.hand = hand;
                    }
                    let update = player.get_update_message(dual_control_player_index);
                    self.messages.add_global_message(update, true, true);
                    if let Some(old_movement) = old_movement {
                        set_view_player_index(old_movement, &mut self.players, old_movement);
                    }
                    if let Some(old_stick) = old_stick {
                        set_view_player_index(old_stick, &mut self.players, old_stick);
                    }
                    if let Some(movement) = movement {
                        set_view_player_index(
                            movement,
                            &mut self.players,
                            dual_control_player_index,
                        );
                        self.move_to_spectator(behaviour, movement);
                    }
                    if let Some(stick) = stick {
                        set_view_player_index(stick, &mut self.players, dual_control_player_index);
                        self.move_to_spectator(behaviour, stick);
                    }
                }
            }
        }
    }

    pub fn get_dual_control_player(
        &self,
        player_index: HQMServerPlayerIndex,
    ) -> Option<(
        HQMServerPlayerIndex,
        Option<HQMServerPlayerIndex>,
        Option<HQMServerPlayerIndex>,
    )> {
        for (i, player) in self.players.iter() {
            if let Some(player) = player {
                if let HQMServerPlayerData::DualControl { movement, stick } = player.data {
                    if movement == Some(player_index) || stick == Some(player_index) {
                        return Some((i, movement, stick));
                    }
                }
            }
        }
        None
    }

    pub fn swap_team(&mut self, player_index: HQMServerPlayerIndex, team: HQMTeam) -> bool {
        if let Some(player) = self.players.get_mut(player_index) {
            if let Some((object_index, _)) = player.object {
                let object = Some((object_index, team));
                player.object = object;
                let update = player.get_update_message(player_index);
                self.messages.add_global_message(update, true, true);
                return true;
            }
        }
        false
    }

    fn add_user_team_message(&mut self, message: &str, sender_index: HQMServerPlayerIndex) {
        if let Some(player) = self.players.get(sender_index) {
            let team = if let Some((_, team)) = player.object {
                Some(team)
            } else if let Some((dual_control_player_index, _, _)) =
                self.get_dual_control_player(sender_index)
            {
                if let Some(dual_control_player) = self.players.get(dual_control_player_index) {
                    if let Some((_, team)) = dual_control_player.object {
                        Some(team)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(team) = team {
                info!(
                    "{} ({}) to team {}: {}",
                    &player.player_name, sender_index, team, message
                );

                let change1 = Rc::new(HQMMessage::PlayerUpdate {
                    player_name: Rc::new(format!("[{}] {}", team, player.player_name)),
                    object: player.object,
                    player_index: sender_index,
                    in_server: true,
                });
                let change2 = Rc::new(HQMMessage::PlayerUpdate {
                    player_name: player.player_name.clone(),
                    object: player.object,
                    player_index: sender_index,
                    in_server: true,
                });
                let chat = Rc::new(HQMMessage::Chat {
                    player_index: Some(sender_index),
                    message: Cow::Owned(message.to_owned()),
                });

                let mut matching_indices = smallvec::SmallVec::<[_; 32]>::new();
                for (player_index, player) in self.players.iter() {
                    if let Some(player) = player {
                        if let Some((_, player_team)) = player.object {
                            if player_team == team {
                                if let HQMServerPlayerData::DualControl { movement, stick } =
                                    player.data
                                {
                                    movement.map(|i| matching_indices.push(i));
                                    stick.map(|i| matching_indices.push(i));
                                } else {
                                    matching_indices.push(player_index);
                                }
                            }
                        }
                    }
                }
                for player_index in matching_indices {
                    if let Some(player) = self.players.get_mut(player_index) {
                        player.add_message(change1.clone());
                        player.add_message(chat.clone());
                        player.add_message(change2.clone());
                    }
                }
            }
        }
    }

    fn find_player_slot(&self, addr: SocketAddr) -> Option<HQMServerPlayerIndex> {
        return self
            .players
            .iter()
            .find(|(_, x)| match x {
                Some(x) => {
                    if let HQMServerPlayerData::NetworkPlayer { data } = &x.data {
                        data.addr == addr
                    } else {
                        false
                    }
                }
                None => false,
            })
            .map(|x| x.0);
    }

    fn find_empty_player_slot(&self) -> Option<HQMServerPlayerIndex> {
        return self.players.iter().find(|(_, x)| x.is_none()).map(|x| x.0);
    }

    fn game_step<B: HQMServerBehaviour>(&mut self, behaviour: &mut B, write_buf: &mut [u8]) {
        self.game.game_step = self.game.game_step.wrapping_add(1);

        behaviour.before_tick(self);

        let mut dual_control_updates = smallvec::SmallVec::<[_; 64]>::new();
        for (player_index, player) in self.players.iter() {
            if let Some(player) = player {
                if let HQMServerPlayerData::DualControl { movement, stick } = &player.data {
                    let mut current_input = player.input.clone();
                    let movement = movement
                        .and_then(|x| self.players.get(x))
                        .map(|x| x.input.clone());
                    let stick = stick
                        .and_then(|x| self.players.get(x))
                        .map(|x| x.input.clone());
                    if let Some(movement) = movement {
                        current_input.fwbw = movement.fwbw;
                        current_input.keys = movement.keys & 0x13;
                        current_input.turn = movement.turn;
                        current_input.head_rot = movement.head_rot;
                        current_input.body_rot = movement.body_rot;
                    }
                    if let Some(stick) = stick {
                        current_input.stick = stick.stick;
                        current_input.stick_angle = stick.stick_angle;
                    }
                    dual_control_updates.push((player_index, current_input))
                }
            }
        }

        for (player_index, new_input) in dual_control_updates {
            self.players
                .get_mut(player_index)
                .map(|x| x.input = new_input);
        }

        for (_, player) in self.players.iter() {
            if let Some(player) = player {
                if let Some((object_index, _)) = player.object {
                    if let Some(skater) = self.game.world.objects.get_skater_mut(object_index) {
                        skater.input = player.input.clone()
                    }
                }
            }
        }

        let events = self.game.world.simulate_step();

        let packets = get_packets(&self.game.world.objects.objects);

        behaviour.after_tick(self, &events);

        if self.game.history_length > 0 {
            let new_replay_tick = ReplayTick {
                game_step: self.game.game_step,
                packets: packets.clone(),
            };

            self.game
                .saved_history
                .truncate(self.game.history_length - 1);
            self.game.saved_history.push_front(new_replay_tick);
        } else {
            self.game.saved_history.clear();
        }

        self.game
            .saved_packets
            .truncate(192 - 1);
        self.game.saved_packets.push_front(packets);
        self.game.packet = self.game.packet.wrapping_add(1);
        self.game
            .saved_pings
            .truncate(100 - 1);
        self.game.saved_pings.push_front(Instant::now());

        if self.config.replays_enabled {
            write_replay(
                &mut self.game,
                &self.messages.get_replay_messages(),
                write_buf,
            );
        }
    }

    fn remove_inactive_players<B: HQMServerBehaviour>(&mut self, behaviour: &mut B) {
        let inactive_players: smallvec::SmallVec<[_; 8]> = self
            .players
            .iter_mut()
            .filter_map(|(player_index, player)| {
                if let Some(player) = player {
                    if let HQMServerPlayerData::NetworkPlayer { data } = &mut player.data {
                        data.inactivity += 1;
                        if data.inactivity > 500 {
                            Some((player_index, player.player_name.clone()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        for (player_index, player_name) in inactive_players {
            self.remove_player(behaviour, player_index, true);
            info!("{} ({}) timed out", player_name, player_index);
            let chat_msg = format!("{} timed out", player_name);
            self.messages.add_server_chat_message(chat_msg);
        }
    }

    async fn tick<B: HQMServerBehaviour>(
        &mut self,
        socket: &UdpSocket,
        behaviour: &mut B,
        write_buf: &mut [u8],
    ) {
        if self.player_count() != 0 {
            self.game.active = true;
            let (game_step, forced_view) = tokio::task::block_in_place(|| {
                self.remove_inactive_players(behaviour);

                if let Some(replay_element) = self.replay_queue.front_mut() {
                    let from = replay_element.from;
                    let saved_history = &self.game.saved_history;
                    let current_game_step = self.game.game_step;
                    let forced_view = replay_element.force_view;
                    let tick = current_game_step.checked_sub(from).and_then(|i| {
                        if let Some(tick) = saved_history.get(i as usize) {
                            Some((tick, replay_element.from))
                        } else if saved_history.len() > 0 {
                            let new_from = current_game_step - (saved_history.len() as u32 - 1);
                            Some((&saved_history[saved_history.len() - 1], new_from))
                        } else {
                            None
                        }
                    });
                    if let Some((tick, p)) = tick {
                        let game_step = tick.game_step;
                        let packets = tick.packets.clone();

                        replay_element.from = p + 1;

                        if replay_element.from >= replay_element.to {
                            self.replay_queue.pop_front();
                        }

                        self.game
                            .saved_packets
                            .truncate(191 - 1);
                        self.game.saved_packets.push_front(packets);
                        self.game
                            .saved_pings
                            .truncate(100 - 1);
                        self.game.saved_pings.push_front(Instant::now());

                        self.game.packet = self.game.packet.wrapping_add(1);
                        (game_step, forced_view)
                    } else {
                        self.replay_queue.pop_front();
                        self.game_step(behaviour, write_buf);
                        (self.game.game_step, None)
                    }
                } else {
                    self.game_step(behaviour, write_buf);
                    (self.game.game_step, None)
                }
            });

            for (rec, message) in self.messages.waiting_messages.drain(..) {
                match rec {
                    HQMWaitingMessageReceiver::All => {
                        for (_, player) in self.players.iter_mut() {
                            if let Some(player) = player {
                                player.add_message(message.clone());
                            }
                        }
                    }
                    HQMWaitingMessageReceiver::Specific(player_index) => {
                        if let Some(player) = self.players.get_mut(player_index) {
                            player.add_message(message);
                        }
                    }
                }
            }

            send_updates(
                self.game_id,
                &self.game.saved_packets,
                game_step,
                self.game.game_over,
                self.game.red_score,
                self.game.blue_score,
                self.game.time,
                self.game.goal_message_timer,
                self.game.period,
                self.game.rules_state,
                self.game.packet,
                &self.players.players,
                socket,
                forced_view,
                write_buf,
            )
            .await;
        } else if self.game.active {
            info!("Game {} abandoned", self.game_id);
            let new_game = behaviour.create_game();
            self.new_game(new_game);
            self.allow_join = true;
        }
    }

    pub fn new_game(&mut self, new_game: HQMGame) {
        let game_id = self.game_id;
        let old_game = std::mem::replace(&mut self.game, new_game);
        self.game_id += 1;
        self.messages.clear();
        info!("New game {} started", self.game_id);

        if self.config.replays_enabled && old_game.period != 0 {
            let time = old_game.start_time.format("%Y-%m-%dT%H%M%S").to_string();
            let file_name = format!("{}.{}.hrp", self.config.server_name, time);
            let replay_data = old_game.replay_data;

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

        let mut messages = smallvec::SmallVec::<[HQMMessage; 32]>::new();
        for (player_index, p) in self.players.players.iter_mut().enumerate() {
            let player_index = HQMServerPlayerIndex(player_index);
            if let Some(player) = p {
                if player.reset(player_index) {
                    let update = player.get_update_message(player_index);
                    messages.push(update);
                } else {
                    let update = HQMMessage::PlayerUpdate {
                        player_name: player.player_name.clone(),
                        object: None,
                        player_index,
                        in_server: false,
                    };
                    self.messages.add_global_message(update, false, false);
                    *p = None;
                }
            }
        }

        self.replay_queue.clear();
    }

    pub fn add_replay_to_queue(
        &mut self,
        start_step: u32,
        end_step: u32,
        force_view: Option<HQMServerPlayerIndex>,
    ) {
        if start_step > end_step {
            panic!("start_packet must be less than or equal to end_packet")
        }

        self.replay_queue.push_back(ReplayElement {
            from: start_step,
            to: end_step,
            force_view,
        });
    }
}

pub(crate) struct ReplayTick {
    game_step: u32,
    packets: smallvec::SmallVec<[HQMObjectPacket; 32]>,
}

pub(crate) struct ReplayElement {
    from: u32,
    to: u32,
    force_view: Option<HQMServerPlayerIndex>,
}

pub async fn run_server<B: HQMServerBehaviour>(
    port: u16,
    public: bool,
    config: HQMServerConfiguration,
    mut behaviour: B,
) -> std::io::Result<()> {
    let mut player_vec = Vec::with_capacity(64);
    for _ in 0..64 {
        player_vec.push(None);
    }
    let first_game = behaviour.create_game();

    let mut server = HQMServer {
        players: HQMServerPlayerList {
            players: player_vec,
        },
        messages: HQMServerMessages::new(),
        ban_list: HashSet::new(),
        allow_join: true,
        game: first_game,
        is_muted: false,
        config,
        game_id: 1,
        replay_queue: VecDeque::new(),
    };
    info!("Server started, new game {} started", 1);

    behaviour.init(&mut server);

    // Set up timers
    let mut tick_timer = tokio::time::interval(Duration::from_millis(10));
    tick_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let socket = Arc::new(tokio::net::UdpSocket::bind(&addr).await?);
    info!(
        "Server listening at address {:?}",
        socket.local_addr().unwrap()
    );

    if public {
        let socket = socket.clone();
        tokio::spawn(async move {
            loop {
                let master_server = get_master_server().await.ok();
                if let Some(addr) = master_server {
                    for _ in 0..60 {
                        let msg = b"Hock\x20";
                        let res = socket.send_to(msg, addr).await;
                        if res.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(10)).await;
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
                        let _ = msg_sender
                            .send(HQMServerReceivedData {
                                addr,
                                data: buf.freeze(),
                            })
                            .await;
                    }
                    Err(_) => {}
                }
            }
        });
    };
    let mut write_buf = [0u8; 4096];
    loop {
        tokio::select! {
            _ = tick_timer.tick() => {
                server.tick(& socket, & mut behaviour, & mut write_buf).await;
            }
            x = msg_receiver.recv() => {
                if let Some (HQMServerReceivedData {
                    addr,
                    data: msg
                }) = x {
                    server.handle_message(addr, & socket, & msg, & mut behaviour, & mut write_buf).await;
                }
            }
        }
    }
}

fn write_message(writer: &mut HQMMessageWriter, message: &HQMMessage) {
    match message {
        HQMMessage::Chat {
            player_index,
            message,
        } => {
            writer.write_bits(6, 2);
            writer.write_bits(
                6,
                match *player_index {
                    Some(x) => x.0 as u32,
                    None => u32::MAX,
                },
            );
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
            assist_player_index,
        } => {
            writer.write_bits(6, 1);
            writer.write_bits(2, team.get_num());
            writer.write_bits(
                6,
                match *goal_player_index {
                    Some(x) => x.0 as u32,
                    None => u32::MAX,
                },
            );
            writer.write_bits(
                6,
                match *assist_player_index {
                    Some(x) => x.0 as u32,
                    None => u32::MAX,
                },
            );
        }
        HQMMessage::PlayerUpdate {
            player_name,
            object,
            player_index,
            in_server,
        } => {
            writer.write_bits(6, 0);
            writer.write_bits(6, player_index.0 as u32);
            writer.write_bits(1, if *in_server { 1 } else { 0 });
            let (object_index, team_num) = match object {
                Some((i, team)) => (i.0 as u32, team.get_num()),
                None => (u32::MAX, u32::MAX),
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

fn write_objects(
    writer: &mut HQMMessageWriter,
    packets: &VecDeque<smallvec::SmallVec<[HQMObjectPacket; 32]>>,
    current_packet: u32,
    known_packet: u32,
) {
    let current_packets = packets[0].as_slice();

    let old_packets = {
        let diff = if known_packet == u32::MAX {
            None
        } else {
            current_packet.checked_sub(known_packet)
        };
        if let Some(diff) = diff {
            let index = diff as usize;
            if index < 192 && index > 0 {
                packets.get(index).map(smallvec::SmallVec::as_slice)
            } else {
                None
            }
        } else {
            None
        }
    };

    writer.write_u32_aligned(current_packet);
    writer.write_u32_aligned(known_packet);

    for i in 0..32 {
        let current_packet = &current_packets[i];
        let old_packet = old_packets.map(|x| &x[i]);
        match current_packet {
            HQMObjectPacket::Puck(puck) => {
                let old_puck = old_packet.and_then(|x| match x {
                    HQMObjectPacket::Puck(old_puck) => Some(old_puck),
                    _ => None,
                });
                writer.write_bits(1, 1);
                writer.write_bits(2, 1); // Puck type
                writer.write_pos(17, puck.pos.0, old_puck.map(|puck| puck.pos.0));
                writer.write_pos(17, puck.pos.1, old_puck.map(|puck| puck.pos.1));
                writer.write_pos(17, puck.pos.2, old_puck.map(|puck| puck.pos.2));
                writer.write_pos(31, puck.rot.0, old_puck.map(|puck| puck.rot.0));
                writer.write_pos(31, puck.rot.1, old_puck.map(|puck| puck.rot.1));
            }
            HQMObjectPacket::Skater(skater) => {
                let old_skater = old_packet.and_then(|x| match x {
                    HQMObjectPacket::Skater(old_skater) => Some(old_skater),
                    _ => None,
                });
                writer.write_bits(1, 1);
                writer.write_bits(2, 0); // Skater type
                writer.write_pos(17, skater.pos.0, old_skater.map(|skater| skater.pos.0));
                writer.write_pos(17, skater.pos.1, old_skater.map(|skater| skater.pos.1));
                writer.write_pos(17, skater.pos.2, old_skater.map(|skater| skater.pos.2));
                writer.write_pos(31, skater.rot.0, old_skater.map(|skater| skater.rot.0));
                writer.write_pos(31, skater.rot.1, old_skater.map(|skater| skater.rot.1));
                writer.write_pos(
                    13,
                    skater.stick_pos.0,
                    old_skater.map(|skater| skater.stick_pos.0),
                );
                writer.write_pos(
                    13,
                    skater.stick_pos.1,
                    old_skater.map(|skater| skater.stick_pos.1),
                );
                writer.write_pos(
                    13,
                    skater.stick_pos.2,
                    old_skater.map(|skater| skater.stick_pos.2),
                );
                writer.write_pos(
                    25,
                    skater.stick_rot.0,
                    old_skater.map(|skater| skater.stick_rot.0),
                );
                writer.write_pos(
                    25,
                    skater.stick_rot.1,
                    old_skater.map(|skater| skater.stick_rot.1),
                );
                writer.write_pos(
                    16,
                    skater.head_rot,
                    old_skater.map(|skater| skater.head_rot),
                );
                writer.write_pos(
                    16,
                    skater.body_rot,
                    old_skater.map(|skater| skater.body_rot),
                );
            }
            HQMObjectPacket::None => {
                writer.write_bits(1, 0);
            }
        }
    }
}

fn write_replay(game: &mut HQMGame, replay_messages: &[Rc<HQMMessage>], write_buf: &mut [u8]) {
    let mut writer = HQMMessageWriter::new(write_buf);

    writer.write_byte_aligned(5);
    writer.write_bits(
        1,
        match game.game_over {
            true => 1,
            false => 0,
        },
    );
    writer.write_bits(8, game.red_score);
    writer.write_bits(8, game.blue_score);
    writer.write_bits(16, game.time);

    writer.write_bits(16, game.goal_message_timer);
    writer.write_bits(8, game.period);

    let packets = &game.saved_packets;

    write_objects(&mut writer, packets, game.packet, game.replay_last_packet);
    game.replay_last_packet = game.packet;

    let remaining_messages = replay_messages.len() - game.replay_msg_pos;

    writer.write_bits(16, remaining_messages as u32);
    writer.write_bits(16, game.replay_msg_pos as u32);

    for message in &replay_messages[game.replay_msg_pos..replay_messages.len()] {
        write_message(&mut writer, Rc::as_ref(message));
    }
    game.replay_msg_pos = replay_messages.len();

    let pos = writer.get_pos();

    let slice = &write_buf[0..pos + 1];

    game.replay_data.extend_from_slice(slice);
}

async fn send_updates(
    game_id: u32,
    packets: &VecDeque<smallvec::SmallVec<[HQMObjectPacket; 32]>>,
    game_step: u32,
    game_over: bool,
    red_score: u32,
    blue_score: u32,
    time: u32,
    goal_message_time: u32,
    period: u32,
    rules_state: HQMRulesState,
    current_packet: u32,
    players: &[Option<HQMServerPlayer>],
    socket: &UdpSocket,
    force_view: Option<HQMServerPlayerIndex>,
    write_buf: &mut [u8],
) {
    for player in players.iter() {
        if let Some(player) = player {
            if let HQMServerPlayerData::NetworkPlayer { data } = &player.data {
                let mut writer = HQMMessageWriter::new(write_buf);

                if data.game_id != game_id {
                    writer.write_bytes_aligned(GAME_HEADER);
                    writer.write_byte_aligned(6);
                    writer.write_u32_aligned(game_id);
                } else {
                    writer.write_bytes_aligned(GAME_HEADER);
                    writer.write_byte_aligned(5);
                    writer.write_u32_aligned(game_id);
                    writer.write_u32_aligned(game_step);
                    writer.write_bits(
                        1,
                        match game_over {
                            true => 1,
                            false => 0,
                        },
                    );
                    writer.write_bits(8, red_score);
                    writer.write_bits(8, blue_score);
                    writer.write_bits(16, time);

                    writer.write_bits(16, goal_message_time);
                    writer.write_bits(8, period);
                    let view = force_view.unwrap_or(data.view_player_index).0 as u32;
                    writer.write_bits(8, view);

                    // if using a non-cryptic version, send ping
                    if data.client_version.has_ping() {
                        writer.write_u32_aligned(data.deltatime);
                    }

                    // if baba's second version or above, send rules
                    if data.client_version.has_rules() {
                        let num = match rules_state {
                            HQMRulesState::Regular {
                                offside_warning,
                                icing_warning,
                            } => {
                                let mut res = 0;
                                if offside_warning {
                                    res |= 1;
                                }
                                if icing_warning {
                                    res |= 2;
                                }
                                res
                            }
                            HQMRulesState::Offside => 4,
                            HQMRulesState::Icing => 8,
                        };
                        writer.write_u32_aligned(num);
                    }

                    write_objects(&mut writer, packets, current_packet, data.known_packet);

                    let (start, remaining_messages) = if data.known_msgpos > data.messages.len() {
                        (data.messages.len(), 0)
                    } else {
                        (
                            data.known_msgpos,
                            min(data.messages.len() - data.known_msgpos, 15),
                        )
                    };

                    writer.write_bits(4, remaining_messages as u32);
                    writer.write_bits(16, start as u32);

                    for message in &data.messages[start..start + remaining_messages] {
                        write_message(&mut writer, Rc::as_ref(message));
                    }
                }
                let bytes_written = writer.get_bytes_written();

                let slice = &write_buf[0..bytes_written];
                let _ = socket.send_to(slice, data.addr).await;
            }
        }
    }
}

fn get_packets(objects: &[HQMGameObject]) -> smallvec::SmallVec<[HQMObjectPacket; 32]> {
    let mut packets = smallvec::SmallVec::<[HQMObjectPacket; 32]>::new();
    for i in 0usize..32 {
        let packet = match &objects[i] {
            HQMGameObject::Puck(puck) => HQMObjectPacket::Puck(puck.get_packet()),
            HQMGameObject::Player(player) => HQMObjectPacket::Skater(player.get_packet()),
            HQMGameObject::None => HQMObjectPacket::None,
        };
        packets.push(packet);
    }
    packets
}

fn get_player_name(bytes: Vec<u8>) -> Option<String> {
    let first_null = bytes.iter().position(|x| *x == 0);

    let bytes = match first_null {
        Some(x) => &bytes[0..x],
        None => &bytes[..],
    }
    .to_vec();
    return match String::from_utf8(bytes) {
        Ok(s) => {
            let s = s.trim();
            let s = if s.is_empty() { "Noname" } else { s };
            Some(String::from(s))
        }
        Err(_) => None,
    };
}

async fn get_master_server() -> Result<SocketAddr, Box<dyn Error>> {
    let s = reqwest::get("http://www.crypticsea.com/anewzero/serverinfo.php")
        .await?
        .text()
        .await?;

    let split = s.split_ascii_whitespace().collect::<Vec<&str>>();

    let addr = split.get(1).unwrap_or(&"").parse::<IpAddr>()?;
    let port = split.get(2).unwrap_or(&"").parse::<u16>()?;
    Ok(SocketAddr::new(addr, port))
}

fn set_view_player_index(
    i: HQMServerPlayerIndex,
    players: &mut HQMServerPlayerList,
    val: HQMServerPlayerIndex,
) {
    if let Some(player) = players.get_mut(i) {
        if let HQMServerPlayerData::NetworkPlayer {
            data: HQMNetworkPlayerData {
                view_player_index, ..
            },
        } = &mut player.data
        {
            *view_player_index = val;
        }
    }
}

pub fn get_spawnpoint(
    rink: &HQMRink,
    team: HQMTeam,
    spawn_point: HQMSpawnPoint,
) -> (Point3<f32>, Rotation3<f32>) {
    match team {
        HQMTeam::Red => match spawn_point {
            HQMSpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) + 3.0, 0.0);
                let pos = Point3::new(rink.width / 2.0, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, rot, 0.0);
                (pos, rot)
            }
            HQMSpawnPoint::Bench => {
                let z = (rink.length / 2.0) + 4.0;
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            }
        },
        HQMTeam::Blue => match spawn_point {
            HQMSpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) - 3.0, PI);
                let pos = Point3::new(rink.width / 2.0, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, rot, 0.0);
                (pos, rot)
            }
            HQMSpawnPoint::Bench => {
                let z = (rink.length / 2.0) - 4.0;
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            }
        },
    }
}

fn get_dual_control_name(
    players: &HQMServerPlayerList,
    movement: Option<HQMServerPlayerIndex>,
    stick: Option<HQMServerPlayerIndex>,
) -> String {
    let s1 = movement
        .and_then(|i| players.get(i))
        .map(|player| player.player_name.as_str())
        .unwrap_or("?");
    let s2 = stick
        .and_then(|i| players.get(i))
        .map(|player| player.player_name.as_str())
        .unwrap_or("?");
    format!("{}+{}", s1, s2)
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum HQMMuteStatus {
    NotMuted,
    ShadowMuted,
    Muted,
}
pub struct HQMNetworkPlayerData {
    pub addr: SocketAddr,
    client_version: HQMClientVersion,
    inactivity: u32,
    known_packet: u32,
    known_msgpos: usize,
    chat_rep: Option<u8>,
    deltatime: u32,
    last_ping: VecDeque<f32>,
    view_player_index: HQMServerPlayerIndex,
    pub game_id: u32,
    messages: Vec<Rc<HQMMessage>>,
}

pub enum HQMServerPlayerData {
    NetworkPlayer {
        data: HQMNetworkPlayerData,
    },
    DualControl {
        movement: Option<HQMServerPlayerIndex>,
        stick: Option<HQMServerPlayerIndex>,
    },
}

pub struct HQMServerPlayer {
    pub player_name: Rc<String>,
    pub object: Option<(HQMObjectIndex, HQMTeam)>,
    pub id: Uuid,
    pub data: HQMServerPlayerData,
    pub is_admin: bool,
    pub is_muted: HQMMuteStatus,
    pub hand: HQMSkaterHand,
    pub mass: f32,
    pub input: HQMPlayerInput,
}

impl HQMServerPlayer {
    pub fn new_network_player(
        player_index: HQMServerPlayerIndex,
        player_name: String,
        addr: SocketAddr,
        global_messages: &[Rc<HQMMessage>],
    ) -> Self {
        HQMServerPlayer {
            player_name: Rc::new(player_name),
            object: None,
            id: Uuid::new_v4(),
            data: HQMServerPlayerData::NetworkPlayer {
                data: HQMNetworkPlayerData {
                    addr,
                    client_version: HQMClientVersion::Vanilla,
                    inactivity: 0,
                    known_packet: u32::MAX,
                    known_msgpos: 0,
                    chat_rep: None,
                    // store latest deltime client sends you to respond with it
                    deltatime: 0,
                    last_ping: VecDeque::new(),
                    view_player_index: player_index,
                    game_id: u32::MAX,
                    messages: global_messages.into_iter().cloned().collect(),
                },
            },
            is_admin: false,
            input: Default::default(),
            is_muted: HQMMuteStatus::NotMuted,
            hand: HQMSkaterHand::Right,
            mass: 1.0,
        }
    }

    fn reset(&mut self, player_index: HQMServerPlayerIndex) -> bool {
        self.object = None;
        if let HQMServerPlayerData::NetworkPlayer { data } = &mut self.data {
            data.known_msgpos = 0;
            data.known_packet = u32::MAX;
            data.messages.clear();
            data.view_player_index = player_index;
        } else if let HQMServerPlayerData::DualControl { .. } = &mut self.data {
            return false;
        }
        return true;
    }

    fn get_update_message(&self, player_index: HQMServerPlayerIndex) -> HQMMessage {
        HQMMessage::PlayerUpdate {
            player_name: self.player_name.clone(),
            object: self.object,
            player_index,
            in_server: true,
        }
    }

    fn add_message(&mut self, message: Rc<HQMMessage>) {
        match &mut self.data {
            HQMServerPlayerData::NetworkPlayer {
                data: HQMNetworkPlayerData { messages, .. },
            } => {
                messages.push(message);
            }
            _ => {}
        }
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        match self.data {
            HQMServerPlayerData::NetworkPlayer {
                data: HQMNetworkPlayerData { addr, .. },
            } => Some(addr),
            _ => None,
        }
    }

    pub fn ping_data(&self) -> Option<PingData> {
        match self.data {
            HQMServerPlayerData::NetworkPlayer {
                data: HQMNetworkPlayerData { ref last_ping, .. },
            } => {
                let n = last_ping.len() as f32;
                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;
                let mut sum = 0f32;
                for i in last_ping.iter() {
                    min = min.min(*i);
                    max = max.max(*i);
                    sum += *i;
                }
                let avg = sum / n;
                let dev = {
                    let mut s = 0f32;
                    for i in last_ping.iter() {
                        s += (*i - avg).powi(2);
                    }
                    (s / n).sqrt()
                };
                Some(PingData {
                    min,
                    max,
                    avg,
                    deviation: dev,
                })
            }
            _ => None,
        }
    }
}

#[derive(Copy, Clone)]
pub struct PingData {
    pub min: f32,
    pub max: f32,
    pub avg: f32,
    pub deviation: f32,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMSpawnPoint {
    Center,
    Bench,
}

#[derive(Debug, Clone)]
pub struct HQMServerConfiguration {
    pub welcome: Vec<String>,
    pub password: String,
    pub player_max: usize,

    pub replays_enabled: bool,
    pub server_name: String,
}

pub trait HQMServerBehaviour {
    fn init(&mut self, _server: &mut HQMServer) {}

    fn before_tick(&mut self, server: &mut HQMServer);
    fn after_tick(&mut self, server: &mut HQMServer, events: &[HQMSimulationEvent]);
    fn handle_command(
        &mut self,
        _server: &mut HQMServer,
        _cmd: &str,
        _arg: &str,
        _player_index: HQMServerPlayerIndex,
    ) {
    }

    fn create_game(&mut self) -> HQMGame;

    fn before_player_exit(&mut self, _server: &mut HQMServer, _player_index: HQMServerPlayerIndex) {
    }

    fn after_player_join(&mut self, _server: &mut HQMServer, _player_index: HQMServerPlayerIndex) {}

    fn get_number_of_players(&self) -> u32;
}

#[derive(Debug, Clone)]
pub enum HQMObjectPacket {
    None,
    Puck(HQMPuckPacket),
    Skater(HQMSkaterPacket),
}

#[derive(Debug, Clone)]
pub struct HQMSkaterPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
    pub stick_pos: (u32, u32, u32),
    pub stick_rot: (u32, u32),
    pub head_rot: u32,
    pub body_rot: u32,
}

#[derive(Debug, Clone)]
pub struct HQMPuckPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
}
