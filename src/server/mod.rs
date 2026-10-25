use std::borrow::Cow;
use std::fmt::Debug;
use std::net::{IpAddr, SocketAddr};

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use futures::{SinkExt, StreamExt};
use jiff::Timestamp;

use glam::Vec3;
use glamx::Rot3;
use std::error::Error;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::udp::UdpFramed;
use tracing::{info, warn};

use crate::gamemode::{CommandResult, ExitReason, GameContext, GameMode};

use self::ban::{BanCheck, BanCheckResponse};
use self::recording::RecordingSaveMethod;
use crate::game::{
    PhysicsConfiguration, PlayerId, PlayerIndex, Puck, PuckId, Rink, ScoreboardValues, SkaterHand,
    SkaterObject, Team,
};
use crate::protocol::{
    HQMClientToServerMessage, HQMMasterServerRegistration, HQMMessageCodec, HQMMessageWriter,
    HQMServerToClientMessage, HQMServerUpdate, ObjectPacket, PlayerUpdate, write_message,
    write_objects,
};
use crate::{ReplayRecording, ServerConfiguration};
pub(crate) use players::{
    HQMMessage, MessageRecording, MessageRetention, MuteStatus, PlayerListExt, PlayerSlots,
    ServerMessages, ServerPlayerData, ServerPlayersAndMessages,
};

type HQMFramedSocket = UdpFramed<HQMMessageCodec, Arc<UdpSocket>>;

mod admin_commands;
pub mod ban;
pub(crate) mod players;
pub mod recording;
mod replay;
mod replay_players;
mod replication;
#[cfg(test)]
pub(crate) mod test_server;

pub use replay::HQMTickHistory;
use replay::{SkaterOwner, SkaterOwners};
use replay_players::ReplayPlayers;
use replication::ReplicationState;
pub(crate) use replication::{PacketHistory, PacketNumber};

#[cfg(test)]
use replication::PACKET_HISTORY_LEN;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObjectSlot(usize);

impl ObjectSlot {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum HQMClientVersion {
    Vanilla,
    Ping,
    PingRules,
}

impl HQMClientVersion {
    pub(crate) fn has_ping(self) -> bool {
        match self {
            HQMClientVersion::Vanilla => false,
            HQMClientVersion::Ping => true,
            HQMClientVersion::PingRules => true,
        }
    }

    pub(crate) fn has_rules(self) -> bool {
        match self {
            HQMClientVersion::Vanilla => false,
            HQMClientVersion::Ping => false,
            HQMClientVersion::PingRules => true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum GameObject {
    Skater(PlayerId, SkaterObject),
    Puck(PuckId, Puck),
}

pub(crate) trait ObjectExt {
    fn spawn_puck(&mut self, puck: Puck, puck_counter: &mut u32) -> Option<PuckId>;
    fn spawn_skater(&mut self, player_id: PlayerId, skater: SkaterObject) -> Option<ObjectSlot>;
}

impl ObjectExt for [Option<GameObject>] {
    fn spawn_puck(&mut self, puck: Puck, puck_counter: &mut u32) -> Option<PuckId> {
        let index = self.iter().position(|x| x.is_none())?;
        let puck_id = PuckId {
            index,
            counter: *puck_counter,
        };
        *puck_counter = puck_counter.wrapping_add(1);
        self[index] = Some(GameObject::Puck(puck_id, puck));
        Some(puck_id)
    }

    fn spawn_skater(&mut self, player_id: PlayerId, skater: SkaterObject) -> Option<ObjectSlot> {
        if let Some(object_index) = self.iter().position(|x| x.is_none()) {
            self[object_index] = Some(GameObject::Skater(player_id, skater));
            Some(ObjectSlot(object_index))
        } else {
            None
        }
    }
}

struct RecordingState {
    data: BytesMut,
    message_pos: usize,
    last_packet: Option<PacketNumber>,
}

impl RecordingState {
    fn new() -> Self {
        Self {
            data: BytesMut::with_capacity(64 * 1024 * 1024),
            message_pos: 0,
            last_packet: None,
        }
    }

    fn new_game(&mut self) {
        self.data.clear();
        self.message_pos = 0;
        self.last_packet = None;
    }

    fn take_data(&mut self) -> BytesMut {
        std::mem::replace(&mut self.data, BytesMut::new())
    }
}

pub(crate) struct HQMServerState {
    pub(crate) player_message_state: ServerPlayersAndMessages,

    pub(crate) objects: Vec<Option<GameObject>>,
    /// Counter for [PuckId], so that every spawned puck gets a new ID.
    pub(crate) puck_counter: u32,

    pub(crate) replay: HQMTickHistory,
    replay_players: ReplayPlayers,
    replication: ReplicationState,
    recording: RecordingState,
}

impl HQMServerState {
    pub(crate) fn new() -> Self {
        Self {
            player_message_state: ServerPlayersAndMessages::new(),
            objects: vec![None; 32],
            puck_counter: 0,
            replay: HQMTickHistory::new(),
            replay_players: ReplayPlayers::new(),
            replication: ReplicationState::new(),
            recording: RecordingState::new(),
        }
    }

    /// Shows the player list for a replayed tick with the given skater owners, or for the live
    /// game if `owners` is `None`.
    fn show_replay_players(&mut self, owners: Option<&SkaterOwners>) {
        let ServerPlayersAndMessages { players, messages } = &mut self.player_message_state;
        self.replay_players.update(players, messages, owners);
    }

    /// The owners of the skater objects, by object slot.
    fn skater_owners(&self) -> SkaterOwners {
        let players = &self.player_message_state.players;
        std::array::from_fn(|slot| match self.objects.get(slot)? {
            Some(GameObject::Skater(player_id, _)) => {
                let player = players.get_player(*player_id)?;
                Some(SkaterOwner {
                    player_id: *player_id,
                    player_name: player.player_name.clone(),
                    team: player.team()?,
                })
            }
            _ => None,
        })
    }

    pub(crate) fn skater_links(&mut self) -> SkaterLinks<'_> {
        SkaterLinks {
            players: &mut self.player_message_state.players,
            messages: &mut self.player_message_state.messages,
            objects: &mut self.objects,
        }
    }

    pub fn set_hand(
        &mut self,
        hand: SkaterHand,
        player_id: PlayerId,
        change_allowed: bool,
    ) -> bool {
        self.skater_links()
            .set_hand(hand, player_id, change_allowed)
    }

    pub fn remove_player(&mut self, player_id: PlayerId, on_recording: bool) -> bool {
        self.skater_links().remove_player(player_id, on_recording)
    }
}

/// The parts of the server state that link players to their skater objects.
///
/// Operations that create or remove skaters, or remove players, go through this, so the link
/// stored on the player and the owner stored on the skater object always match.
pub(crate) struct SkaterLinks<'a> {
    pub(crate) players: &'a mut PlayerSlots,
    pub(crate) messages: &'a mut ServerMessages,
    pub(crate) objects: &'a mut [Option<GameObject>],
}

impl SkaterLinks<'_> {
    pub(crate) fn skater_for_player_mut(
        &mut self,
        player_id: PlayerId,
    ) -> Option<&mut SkaterObject> {
        let object_slot = self.players.get_player(player_id)?.skater_assignment()?.0;
        match self.objects.get_mut(object_slot.index())? {
            Some(GameObject::Skater(object_player_id, skater))
                if *object_player_id == player_id =>
            {
                Some(skater)
            }
            _ => None,
        }
    }

    fn clear_skater_assignment(&mut self, player_id: PlayerId) -> bool {
        let object_slot = match self
            .players
            .get_player(player_id)
            .and_then(|player| player.skater_assignment())
        {
            Some((object_slot, _)) => object_slot,
            None => return false,
        };

        let Some(GameObject::Skater(object_player_id, _)) = self
            .objects
            .get(object_slot.index())
            .and_then(Option::as_ref)
        else {
            return false;
        };
        if *object_player_id != player_id {
            return false;
        }

        self.objects[object_slot.index()] = None;
        if let Some(player) = self.players.get_player_mut(player_id) {
            player.clear_skater_assignment();
            true
        } else {
            false
        }
    }

    fn broadcast_player_update(&mut self, player_id: PlayerId) {
        if let Some(player) = self.players.get_player(player_id) {
            let update = player.get_update_message(player_id.index);
            self.messages.broadcast_message(
                update,
                MessageRetention::ForLateJoiners,
                MessageRecording::Include,
            );
        }
    }

    /// Sets the player's preferred hand, and applies it if the player may change hands now.
    ///
    /// Returns whether the player now plays with the preferred hand.
    pub(crate) fn set_hand(
        &mut self,
        hand: SkaterHand,
        player_id: PlayerId,
        change_allowed: bool,
    ) -> bool {
        if let Some(player) = self.players.get_player_mut(player_id) {
            player.preferred_hand = hand;
        }
        self.update_hand(player_id, change_allowed)
    }

    /// Applies the preferred hand of every player who may change hands now.
    pub(crate) fn update_hands(&mut self, change_allowed: bool) {
        let player_ids: smallvec::SmallVec<[_; 32]> =
            self.players.iter_players().map(|(id, _)| id).collect();
        for player_id in player_ids {
            self.update_hand(player_id, change_allowed);
        }
    }

    /// Applies the player's preferred hand unless the player has been on the ice since hand
    /// changes were last allowed, and returns whether the player now plays with it.
    fn update_hand(&mut self, player_id: PlayerId, change_allowed: bool) -> bool {
        let on_ice = self.skater_for_player_mut(player_id).is_some();
        let Some(player) = self.players.get_player_mut(player_id) else {
            return false;
        };
        if change_allowed {
            player.hand_locked = false;
        } else if on_ice {
            player.hand_locked = true;
        }
        let hand = player.preferred_hand;
        if player.hand != hand && !player.hand_locked {
            player.hand = hand;
            if let Some(skater) = self.skater_for_player_mut(player_id) {
                skater.hand = hand;
            }
        }
        self.players
            .get_player(player_id)
            .is_some_and(|player| player.hand == player.preferred_hand)
    }

    pub(crate) fn move_to_spectator(&mut self, player_id: PlayerId) -> bool {
        if self.clear_skater_assignment(player_id) {
            self.broadcast_player_update(player_id);
            true
        } else {
            false
        }
    }

    pub(crate) fn spawn_skater(
        &mut self,
        player_id: PlayerId,
        team: Team,
        pos: Vec3,
        rot: Rot3,
        keep_stick_position: bool,
    ) -> bool {
        let Some(player) = self.players.get_player(player_id) else {
            return false;
        };
        let hand = player.hand;
        let has_assignment = player.skater_assignment().is_some();

        if has_assignment {
            let Some(skater) = self.skater_for_player_mut(player_id) else {
                warn!(?player_id, "player skater assignment is invalid");
                return false;
            };
            let mut new_skater = SkaterObject::new(pos, rot, hand);
            if keep_stick_position {
                let stick_pos_diff = skater.stick_pos - skater.body.pos;
                let frame_change = rot * skater.body.rot.inverse();
                new_skater.stick_pos = pos + frame_change * stick_pos_diff;
                new_skater.stick_rot = frame_change * skater.stick_rot;
                new_skater.stick_placement = skater.stick_placement;
                new_skater.stick_placement_delta = skater.stick_placement_delta;
            }
            *skater = new_skater;
            self.players
                .get_player_mut(player_id)
                .expect("player must exist while updating its skater")
                .set_skater_team(team);
        } else {
            let skater = SkaterObject::new(pos, rot, hand);
            let Some(object_slot) = self.objects.spawn_skater(player_id, skater) else {
                return false;
            };
            let player = self
                .players
                .get_player_mut(player_id)
                .expect("player must exist while spawning its skater");
            player.set_skater_assignment(object_slot, team);
            if let ServerPlayerData::NetworkPlayer { data } = &mut player.data {
                data.view_player_index = player_id.index;
            }
        }

        self.broadcast_player_update(player_id);
        true
    }

    pub(crate) fn remove_player(&mut self, player_id: PlayerId, on_recording: bool) -> bool {
        if let Some(player) = self.players.get_player(player_id) {
            let update = HQMMessage::PlayerUpdate {
                player_index: player_id.index,
                data: None,
            };
            let has_skater = player.has_skater();

            if has_skater && !self.clear_skater_assignment(player_id) {
                warn!(
                    ?player_id,
                    "player skater assignment is invalid during player removal"
                );
            }

            self.players
                .remove(player_id)
                .expect("player must exist while removing it");

            self.messages.broadcast_message(
                update,
                MessageRetention::ForLateJoiners,
                if on_recording {
                    MessageRecording::Include
                } else {
                    MessageRecording::Exclude
                },
            );

            true
        } else {
            false
        }
    }
}

/// The server state that game modes can reach, split into parts that can be borrowed
/// independently.
pub(crate) struct ServerSplit<'a> {
    pub(crate) players: &'a mut PlayerSlots,
    pub(crate) messages: &'a mut ServerMessages,
    pub(crate) objects: &'a mut [Option<GameObject>],
    pub(crate) puck_counter: &'a mut u32,
    pub(crate) replay: &'a mut HQMTickHistory,
    pub(crate) rink: &'a mut Rink,
    pub(crate) config: &'a mut ServerConfiguration,
    pub(crate) physics_config: &'a mut PhysicsConfiguration,
    pub(crate) new_game_requested: &'a mut bool,
}

pub(crate) struct HQMServer {
    pub(crate) state: HQMServerState,

    pub config: ServerConfiguration,

    pub physics_config: PhysicsConfiguration,
    pub rink: Rink,

    game_id: u32,
    pub is_muted: bool,
    pub start_time: Timestamp,

    has_current_game_been_active: bool,
    /// Whether a game mode asked for a new game with [GameContext::new_game], which
    /// [HQMServer::start_requested_game] carries out.
    new_game_requested: bool,

    pub(crate) ban: Box<dyn BanCheck>,
    pub(crate) save_recording: Box<dyn RecordingSaveMethod>,
}

impl HQMServer {
    pub(crate) fn new(
        config: ServerConfiguration,
        physics_config: PhysicsConfiguration,
        ban: Box<dyn BanCheck>,
        save_recording: Box<dyn RecordingSaveMethod>,
    ) -> Self {
        HQMServer {
            state: HQMServerState::new(),
            physics_config,
            is_muted: false,
            config,
            game_id: 1,

            has_current_game_been_active: false,
            new_game_requested: false,
            ban,
            save_recording,

            start_time: Default::default(),
            rink: Rink::new(30.0, 61.0, 8.5),
        }
    }

    pub(crate) async fn handle_message<B: GameMode>(
        &mut self,
        addr: SocketAddr,
        socket: &mut HQMFramedSocket,
        command: HQMClientToServerMessage,
        behaviour: &mut B,
    ) {
        match command {
            HQMClientToServerMessage::Join {
                version,
                player_name,
            } => {
                self.player_join(addr, version, player_name, behaviour);
            }
            HQMClientToServerMessage::Update(update) => self.player_update(addr, update, behaviour),
            HQMClientToServerMessage::Exit => self.player_exit(addr, behaviour),
            HQMClientToServerMessage::ServerInfo { version, ping } => {
                self.request_info(socket, addr, version, ping, behaviour)
                    .await;
            }
        }
    }

    async fn request_info<B: GameMode>(
        &self,
        socket: &mut HQMFramedSocket,
        addr: SocketAddr,
        _version: u32,
        ping: u32,
        behaviour: &B,
    ) {
        let message = HQMServerToClientMessage::ServerInfo {
            ping,
            player_count: self.real_player_count(),
            team_size: behaviour.server_list_team_size(),
            server_name: self.config.server_name.as_ref(),
        };
        let _ = socket.send((message, addr)).await;
    }

    fn real_player_count(&self) -> usize {
        let mut player_count = 0;
        for (_, player) in self.state.player_message_state.players.iter_players() {
            let is_actual_player = match player.data {
                ServerPlayerData::NetworkPlayer { .. } => true,
                ServerPlayerData::Bot { .. } => false,
            };
            if is_actual_player {
                player_count += 1;
            }
        }
        player_count
    }

    fn player_update<B: GameMode>(
        &mut self,
        addr: SocketAddr,
        update: PlayerUpdate,
        behaviour: &mut B,
    ) {
        let PlayerUpdate {
            current_game_id,
            input,
            deltatime,
            new_known_packet,
            known_msg_pos: known_msgpos,
            chat,
            version: client_version,
        } = update;
        let (player_id, player) = match self
            .state
            .player_message_state
            .players
            .find_player_by_addr_mut(addr)
        {
            Some(x) => x,
            None => {
                return;
            }
        };
        if let ServerPlayerData::NetworkPlayer { data } = &mut player.data {
            let time_received = Instant::now();
            let new_known_packet = PacketNumber::from_wire(new_known_packet);

            let duration_since_packet = match new_known_packet {
                Some(new_known_packet)
                    if data.game_id == current_game_id
                        && data.known_packet.is_none_or(|known_packet| {
                            new_known_packet.is_newer_than(known_packet)
                        }) =>
                {
                    self.state
                        .replication
                        .history
                        .sent_at(new_known_packet)
                        .and_then(|sent_at| time_received.checked_duration_since(sent_at))
                }
                _ => None,
            };

            if let Some(duration_since_packet) = duration_since_packet {
                data.last_ping.push(duration_since_packet.as_secs_f32());
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

            if let Some((rep, message)) = chat
                && data.chat_rep != Some(rep)
            {
                data.chat_rep = Some(rep);
                self.process_message(message, player_id, behaviour);
            }
        }
    }

    fn player_join<B: GameMode>(
        &mut self,
        addr: SocketAddr,
        player_version: u32,
        name: String,
        behaviour: &mut B,
    ) {
        let player_count = self.real_player_count();
        let max_player_count = self.config.player_max;
        if player_count >= max_player_count {
            return; // Ignore join request
        }
        if player_version != 55 {
            return; // Not the right version
        }
        let current_slot = self
            .state
            .player_message_state
            .players
            .find_player_by_addr(addr);
        if current_slot.is_some() {
            return; // Player has already joined
        }

        // Check ban list
        if self.ban.check_ip_banned(addr.ip()) != BanCheckResponse::Allowed {
            return;
        }

        if let Some(player_id) = self.add_player(&name, addr) {
            behaviour.after_player_join(&mut self.context(), player_id);
            info!(
                "{} ({}) joined server from address {:?}",
                name, player_id, addr
            );
            let msg = format!("{name} joined");
            self.state.player_message_state.add_server_chat_message(msg);
            self.start_requested_game(behaviour);
        }
    }

    fn process_command<B: GameMode>(
        &mut self,
        command: &str,
        arg: &str,
        player_id: PlayerId,
        behaviour: &mut B,
    ) {
        match parse_chat_command(command, arg) {
            ChatCommand::Builtin(BuiltinCommand::Mute(Some(player_index))) => {
                self.mute_player(player_id, player_index);
            }
            ChatCommand::Builtin(BuiltinCommand::Unmute(Some(player_index))) => {
                self.unmute_player(player_id, player_index);
            }
            ChatCommand::Builtin(BuiltinCommand::MuteChat) => self.mute_chat(player_id),
            ChatCommand::Builtin(BuiltinCommand::UnmuteChat) => self.unmute_chat(player_id),
            ChatCommand::Builtin(BuiltinCommand::Kick(Some(player_index))) => {
                self.kick_player(player_id, player_index, false, behaviour);
            }
            ChatCommand::Builtin(BuiltinCommand::KickAll(name)) => {
                self.kick_all_matching(player_id, name, false, behaviour);
            }
            ChatCommand::Builtin(BuiltinCommand::Ban(Some(player_index))) => {
                self.kick_player(player_id, player_index, true, behaviour);
            }
            ChatCommand::Builtin(BuiltinCommand::BanAll(name)) => {
                self.kick_all_matching(player_id, name, true, behaviour);
            }
            ChatCommand::Builtin(BuiltinCommand::ClearBans) => self.clear_bans(player_id),
            ChatCommand::Builtin(BuiltinCommand::Reset) => self.reset_game(player_id),
            ChatCommand::Builtin(BuiltinCommand::SetRecording(rule)) => {
                self.set_recording(player_id, rule);
            }
            ChatCommand::Builtin(BuiltinCommand::Lefty) => {
                self.set_hand(SkaterHand::Left, player_id, behaviour);
            }
            ChatCommand::Builtin(BuiltinCommand::Righty) => {
                self.set_hand(SkaterHand::Right, player_id, behaviour);
            }
            ChatCommand::Builtin(BuiltinCommand::Admin(password)) => {
                self.admin_login(player_id, password);
            }
            ChatCommand::Builtin(BuiltinCommand::RestartServer) => self.restart_server(player_id),
            ChatCommand::Builtin(BuiltinCommand::List(Some(first_index))) => {
                self.list_players(player_id, first_index);
            }
            ChatCommand::Builtin(BuiltinCommand::Search(name)) => {
                self.search_players(player_id, name)
            }
            ChatCommand::Builtin(BuiltinCommand::Ping(Some(player_index))) => {
                self.ping(player_index, player_id);
            }
            ChatCommand::Builtin(BuiltinCommand::PingByName(name)) => {
                self.ping_by_name(player_id, name);
            }
            ChatCommand::Builtin(BuiltinCommand::View(Some(player_index))) => {
                self.view(player_index, player_id);
            }
            ChatCommand::Builtin(BuiltinCommand::ViewByName(name)) => {
                self.view_by_name(player_id, name);
            }
            ChatCommand::Builtin(BuiltinCommand::RestoreView) => {
                if let Some(player) = self
                    .state
                    .player_message_state
                    .players
                    .get_player_mut(player_id)
                    && let ServerPlayerData::NetworkPlayer { data } = &mut player.data
                    && data.view_player_index != player_id.index
                {
                    data.view_player_index = player_id.index;
                    self.state
                        .player_message_state
                        .add_directed_server_chat_message("View has been restored", player_id);
                }
            }
            ChatCommand::Builtin(BuiltinCommand::TeamChat(message)) => {
                self.state
                    .player_message_state
                    .add_user_team_message(message, player_id);
            }
            ChatCommand::Builtin(BuiltinCommand::Version) => {
                let version = env!("CARGO_PKG_VERSION");
                let s = format!("Migo HQM Server, version {version}");

                self.state
                    .player_message_state
                    .add_directed_server_chat_message(s, player_id);
            }
            ChatCommand::Builtin(BuiltinCommand::Git) => {
                let git_sha = option_env!("VERGEN_GIT_SHA");
                let s: Cow<'static, str> = if let Some(git_sha) = git_sha {
                    format!("Git commit: {git_sha}").into()
                } else {
                    "No git commit ID found".into()
                };
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(s, player_id);
            }
            ChatCommand::Builtin(_) => {}
            ChatCommand::GameMode { command, arg } => {
                let result = behaviour.handle_command(&mut self.context(), command, arg, player_id);
                if result == CommandResult::Unknown {
                    self.state
                        .player_message_state
                        .add_directed_server_chat_message(
                            format!("Unknown command /{command}"),
                            player_id,
                        );
                }
            }
        }
    }

    fn set_hand<B: GameMode>(&mut self, hand: SkaterHand, player_id: PlayerId, behaviour: &B) {
        if !self
            .state
            .set_hand(hand, player_id, behaviour.hand_change_allowed())
        {
            self.state
                .player_message_state
                .add_directed_server_chat_message(
                    "Stick hand will change when play has stopped",
                    player_id,
                );
        }
    }

    fn list_players(&mut self, receiver_id: PlayerId, first_index: usize) {
        let res: Vec<_> = self
            .state
            .player_message_state
            .players
            .iter_players()
            .filter(|(x, _)| x.index.0 >= first_index)
            .take(5)
            .map(|(player_index, player)| format!("{}: {}", player_index.index, player.player_name))
            .collect();
        for msg in res {
            self.state
                .player_message_state
                .add_directed_server_chat_message(msg, receiver_id);
        }
    }

    fn search_players(&mut self, player_id: PlayerId, name: &str) {
        let matches = self.player_search(name);
        if matches.is_empty() {
            self.state
                .player_message_state
                .add_directed_server_chat_message("No matches found", player_id);
            return;
        }
        for (found_player_id, found_player_name) in matches.into_iter().take(5) {
            let msg = format!("{}: {}", found_player_id.index, found_player_name);
            self.state
                .player_message_state
                .add_directed_server_chat_message(msg, player_id);
        }
    }

    fn view(&mut self, view_player_index: PlayerIndex, player_id: PlayerId) {
        if let Some((view_player_id, view_player)) = self
            .state
            .player_message_state
            .players
            .get_player_by_index(view_player_index)
        {
            let view_player_name = view_player.player_name.clone();

            // A player on the ice becomes a spectator to view someone else.
            if player_id != view_player_id {
                self.state.skater_links().move_to_spectator(player_id);
            }

            if let Some(player) = self
                .state
                .player_message_state
                .players
                .get_player_mut(player_id)
                && let ServerPlayerData::NetworkPlayer { data } = &mut player.data
                && view_player_index != data.view_player_index
            {
                data.view_player_index = view_player_id.index;
                if player_id != view_player_id {
                    let msg = format!("You are now viewing {view_player_name}");
                    self.state
                        .player_message_state
                        .add_directed_server_chat_message(msg, player_id);
                } else {
                    self.state
                        .player_message_state
                        .add_directed_server_chat_message("View has been restored", player_id);
                }
            }
        } else {
            self.state
                .player_message_state
                .add_directed_server_chat_message("No player with this ID exists", player_id);
        }
    }

    fn ping(&mut self, ping_player_index: PlayerIndex, player_id: PlayerId) {
        if let Some((_, ping_player)) = self
            .state
            .player_message_state
            .players
            .get_player_by_index(ping_player_index)
        {
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
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(msg1, player_id);
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(msg2, player_id);
            } else {
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(
                        "This player is not a connected player",
                        player_id,
                    );
            }
        } else {
            self.state
                .player_message_state
                .add_directed_server_chat_message("No player with this ID exists", player_id);
        }
    }

    pub fn player_exact_unique_match(&self, name: &str) -> Option<(PlayerId, Rc<str>)> {
        let mut found = None;
        for (player_id, player) in self.state.player_message_state.players.iter_players() {
            if player.player_name.as_ref() == name {
                if found.is_none() {
                    found = Some((player_id, player.player_name.clone()));
                } else {
                    return None;
                }
            }
        }
        found
    }

    pub fn player_search(&self, name: &str) -> Vec<(PlayerId, Rc<str>)> {
        let name = name.to_lowercase();
        let mut found = Vec::new();
        for (player_index, player) in self.state.player_message_state.players.iter_players() {
            if player.player_name.to_lowercase().contains(&name) {
                found.push((player_index, player.player_name.clone()));
                if found.len() >= 5 {
                    break;
                }
            }
        }
        found
    }

    fn player_index_from_name(
        &mut self,
        receiver_id: PlayerId,
        name: &str,
        command: &str,
    ) -> Option<PlayerIndex> {
        if let Some((player_id, _)) = self.player_exact_unique_match(name) {
            return Some(player_id.index);
        }

        let matches = self.player_search(name);
        match matches.as_slice() {
            [] => {
                self.state
                    .player_message_state
                    .add_directed_server_chat_message("No matches found", receiver_id);
                None
            }
            [(player_id, _)] => Some(player_id.index),
            _ => {
                let message = format!("Multiple matches found, use /{command} X");
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(message, receiver_id);
                for (player_id, player_name) in matches {
                    let message = format!("{}: {}", player_id.index, player_name);
                    self.state
                        .player_message_state
                        .add_directed_server_chat_message(message, receiver_id);
                }
                None
            }
        }
    }

    fn ping_by_name(&mut self, player_id: PlayerId, name: &str) {
        if let Some(ping_player_index) = self.player_index_from_name(player_id, name, "ping") {
            self.ping(ping_player_index, player_id);
        }
    }

    fn view_by_name(&mut self, player_id: PlayerId, name: &str) {
        if let Some(view_player_index) = self.player_index_from_name(player_id, name, "view") {
            self.view(view_player_index, player_id);
        }
    }

    fn process_message<B: GameMode>(
        &mut self,
        msg: String,
        player_id: PlayerId,
        behaviour: &mut B,
    ) {
        if let Some(player) = self
            .state
            .player_message_state
            .players
            .get_player(player_id)
        {
            if msg.starts_with("/") {
                let split: Vec<&str> = msg.splitn(2, " ").collect();
                let command = &split[0][1..];
                let arg = if split.len() < 2 { "" } else { split[1] };
                self.process_command(command, arg, player_id, behaviour);
                self.start_requested_game(behaviour);
            } else if !self.is_muted {
                match player.is_muted {
                    MuteStatus::NotMuted => {
                        info!("{} ({}): {}", &player.player_name, player_id, &msg);
                        self.state
                            .player_message_state
                            .add_user_chat_message(msg, player_id.index);
                    }
                    MuteStatus::ShadowMuted => {
                        self.state
                            .player_message_state
                            .add_directed_user_chat_message(msg, player_id, player_id.index);
                    }
                    MuteStatus::Muted => {}
                }
            }
        }
    }

    fn player_exit<B: GameMode>(&mut self, addr: SocketAddr, behaviour: &mut B) {
        let player_id = self
            .state
            .player_message_state
            .players
            .find_player_by_addr(addr)
            .map(|(player_id, _)| player_id);

        if let Some(player_id) = player_id
            && let Some(player_name) =
                self.disconnect_player(player_id, ExitReason::Disconnected, behaviour)
        {
            info!("{} ({}) exited server", player_name, player_id);
            let msg = format!("{player_name} exited");
            self.state.player_message_state.add_server_chat_message(msg);
        }
        self.start_requested_game(behaviour);
    }

    fn disconnect_player<B: GameMode>(
        &mut self,
        player_id: PlayerId,
        reason: ExitReason,
        behaviour: &mut B,
    ) -> Option<Rc<str>> {
        let player_name = self
            .state
            .player_message_state
            .players
            .get_player(player_id)?
            .player_name
            .clone();
        behaviour.before_player_exit(&mut self.context(), player_id, reason);
        self.state
            .remove_player(player_id, true)
            .then_some(player_name)
    }

    fn add_player(&mut self, player_name: &str, addr: SocketAddr) -> Option<PlayerId> {
        let res = self
            .state
            .player_message_state
            .add_player(player_name, addr);
        if let Some(player_index) = res {
            let welcome = self.config.welcome.clone();
            for welcome_msg in welcome {
                self.state
                    .player_message_state
                    .add_directed_server_chat_message(welcome_msg, player_index);
            }
        }
        res
    }

    fn game_step<B: GameMode>(&mut self, behaviour: &mut B) -> ScoreboardValues {
        self.state.replay.advance_step();

        let events = self.simulate_step();
        let scoreboard = behaviour.after_tick(&mut self.context(), &events);
        self.state.replication.last_scoreboard = Some(scoreboard);

        let packets = Rc::new(self.get_packets());

        let owners = self.state.skater_owners();
        self.state.replay.record_tick(packets.clone(), owners);

        self.state.replication.history.push(packets);

        if self.config.recording_enabled != ReplayRecording::Off
            && behaviour.include_tick_in_recording()
        {
            self.write_recording_tick(&scoreboard);
        }
        scoreboard
    }

    fn get_packets(&self) -> [ObjectPacket; 32] {
        let mut packets = [const { ObjectPacket::None }; 32];
        for (i, object) in self.state.objects.iter().enumerate() {
            packets[i] = match object {
                None => ObjectPacket::None,
                Some(GameObject::Skater(_, skater)) => ObjectPacket::Skater(skater.get_packet()),
                Some(GameObject::Puck(_, puck)) => ObjectPacket::Puck(puck.get_packet()),
            };
        }

        packets
    }

    fn remove_inactive_players<B: GameMode>(&mut self, behaviour: &mut B) {
        let inactive_players: smallvec::SmallVec<[_; 8]> = self
            .state
            .player_message_state
            .players
            .iter_players_mut()
            .filter_map(|(player_id, player)| {
                if let ServerPlayerData::NetworkPlayer { data } = &mut player.data {
                    data.inactivity += 1;
                    if data.inactivity > 500 {
                        Some(player_id)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        for player_id in inactive_players {
            let Some(player_name) =
                self.disconnect_player(player_id, ExitReason::Timeout, behaviour)
            else {
                continue;
            };
            info!("{} ({}) timed out", player_name, player_id);
            let chat_msg = format!("{player_name} timed out");
            self.state
                .player_message_state
                .add_server_chat_message(chat_msg);
        }
    }

    fn advance_tick<B: GameMode>(&mut self, behaviour: &mut B) -> Option<TickOutput> {
        // A new game asked for in after_tick starts here, so that the tick it was asked for in
        // is still sent and recorded as part of the old game.
        self.start_requested_game(behaviour);

        if self.real_player_count() == 0 {
            if self.has_current_game_been_active {
                info!("Game {} abandoned", self.game_id);
                behaviour.game_abandoned(&mut self.context());
                self.new_game();
            }
            return None;
        }

        if !self.has_current_game_been_active {
            self.start_game(behaviour);
        }

        self.remove_inactive_players(behaviour);
        self.start_requested_game(behaviour);
        behaviour.before_tick(&mut self.context());
        self.start_requested_game(behaviour);
        self.state
            .skater_links()
            .update_hands(behaviour.hand_change_allowed());

        let (game_step, forced_view, scoreboard) =
            if let Some((forced_view, tick)) = self.state.replay.pop_replay_tick() {
                let scoreboard = self.state.replication.last_scoreboard.unwrap_or_default();
                self.state.replication.history.push(tick.packets);
                self.state.show_replay_players(Some(&tick.owners));
                let forced_view = forced_view.and_then(|player_id| {
                    self.state
                        .replay_players
                        .shown_index(&self.state.player_message_state.players, player_id)
                });
                (tick.game_step, forced_view, scoreboard)
            } else {
                let scoreboard = self.game_step(behaviour);
                self.state.show_replay_players(None);
                (self.state.replay.game_step(), None, scoreboard)
            };

        Some(TickOutput {
            game_step,
            forced_view,
            scoreboard,
        })
    }

    pub(crate) async fn tick<B: GameMode>(
        &mut self,
        socket: &mut HQMFramedSocket,
        behaviour: &mut B,
    ) {
        let tick_output = self.advance_tick(behaviour);
        self.state.player_message_state.flush_messages();
        if let Some(tick_output) = tick_output {
            send_updates(
                self.game_id,
                &self.state.replication.history,
                tick_output.game_step,
                &tick_output.scoreboard,
                &self.state.player_message_state.players,
                socket,
                tick_output.forced_view,
            )
            .await;
        }
    }

    /// Gets a [GameContext] for passing to game mode hooks.
    pub(crate) fn context(&mut self) -> GameContext<'_> {
        GameContext::from(self.split())
    }

    /// Splits the server state that game modes can reach into parts that can be borrowed
    /// independently.
    pub(crate) fn split(&mut self) -> ServerSplit<'_> {
        let HQMServerState {
            player_message_state: ServerPlayersAndMessages { players, messages },
            objects,
            puck_counter,
            replay,
            ..
        } = &mut self.state;
        ServerSplit {
            players,
            messages,
            objects,
            puck_counter,
            replay,
            rink: &mut self.rink,
            config: &mut self.config,
            physics_config: &mut self.physics_config,
            new_game_requested: &mut self.new_game_requested,
        }
    }

    /// Starts a game and calls [GameMode::game_started].
    fn start_game<B: GameMode>(&mut self, behaviour: &mut B) {
        self.start_time = Timestamp::now();
        self.has_current_game_been_active = true;
        behaviour.game_started(&mut self.context());
        info!("New game {} started", self.game_id);
    }

    /// Carries out a new game that a game mode asked for with [GameContext::new_game].
    ///
    /// This is called once the server is done with the event in which the game mode asked for it,
    /// such as a tick's [GameMode::before_tick], a chat command or a player leaving. The new game
    /// starts right away if there are players, and otherwise on the first tick with a player.
    pub(crate) fn start_requested_game<B: GameMode>(&mut self, behaviour: &mut B) {
        if !self.new_game_requested {
            return;
        }
        info!("Game {} ended", self.game_id);
        self.new_game();
        if self.real_player_count() > 0 {
            self.start_game(behaviour);
        }
    }

    /// Ends the current game and resets the server state.
    pub fn new_game(&mut self) {
        self.game_id += 1;
        self.has_current_game_been_active = false;
        self.new_game_requested = false;

        let old_recording_data = self.state.recording.take_data();
        if self.config.recording_enabled == ReplayRecording::On && !old_recording_data.is_empty() {
            let size = old_recording_data.len();
            let mut recording_data = BytesMut::with_capacity(size + 8);
            recording_data.put_u32_le(0u32);
            recording_data.put_u32_le(size as u32);
            recording_data.put_slice(&old_recording_data);
            let recording_data = recording_data.freeze();
            self.save_recording
                .save_recording_data(&self.config, recording_data, self.start_time);
        }

        let state = &mut self.state;
        let player_message_state = &mut state.player_message_state;
        player_message_state
            .messages
            .new_game(&mut player_message_state.players);
        state
            .replay_players
            .clear(&mut player_message_state.players);
        state.replay.clear();
        state.replication.new_game();
        state.recording.new_game();
        state.objects.fill(None);
    }

    fn write_recording_tick(&mut self, scoreboard: &ScoreboardValues) {
        let messages_to_write = self
            .state
            .player_message_state
            .recording_messages_since(self.state.recording.message_pos);
        let remaining_messages = messages_to_write.len();
        self.state.recording.data.reserve(
            9 // Header, time, score, period, etc.
            + 8 // Position metadata
            + (32*30) // 32 objects that can be at most 30 bytes each
            + 4 // Message metadata
            + remaining_messages * 66, // Chat message can be up to 66 bytes each
        );
        let mut writer = HQMMessageWriter::new(&mut self.state.recording.data);

        writer.write_byte_aligned(5);
        writer.write_bits(
            1,
            match scoreboard.game_over {
                true => 1,
                false => 0,
            },
        );
        writer.write_bits(8, scoreboard.red_score);
        writer.write_bits(8, scoreboard.blue_score);
        writer.write_bits(16, scoreboard.time);

        writer.write_bits(16, scoreboard.goal_message_timer);
        writer.write_bits(8, scoreboard.period); // 8.1

        write_objects(
            &mut writer,
            &self.state.replication.history,
            self.state.recording.last_packet,
        );
        self.state.recording.last_packet = Some(self.state.replication.history.latest_sequence());

        writer.write_bits(16, remaining_messages as u32);
        writer.write_bits(16, self.state.recording.message_pos as u32);

        for message in messages_to_write {
            write_message(&mut writer, Rc::as_ref(message));
        }
        self.state.recording.message_pos =
            self.state.player_message_state.recording_message_count();
        writer.recording_fix();
    }
}

#[derive(Clone, Copy, Debug)]
struct TickOutput {
    game_step: u32,
    forced_view: Option<PlayerIndex>,
    scoreboard: ScoreboardValues,
}

#[derive(Debug, PartialEq, Eq)]
enum BuiltinCommand<'a> {
    Mute(Option<PlayerIndex>),
    Unmute(Option<PlayerIndex>),
    MuteChat,
    UnmuteChat,
    Kick(Option<PlayerIndex>),
    KickAll(&'a str),
    Ban(Option<PlayerIndex>),
    BanAll(&'a str),
    ClearBans,
    Reset,
    SetRecording(&'a str),
    Lefty,
    Righty,
    Admin(&'a str),
    RestartServer,
    List(Option<usize>),
    Search(&'a str),
    Ping(Option<PlayerIndex>),
    PingByName(&'a str),
    View(Option<PlayerIndex>),
    ViewByName(&'a str),
    RestoreView,
    TeamChat(&'a str),
    Version,
    Git,
}

#[derive(Debug, PartialEq, Eq)]
enum ChatCommand<'a> {
    Builtin(BuiltinCommand<'a>),
    GameMode { command: &'a str, arg: &'a str },
}

fn parse_chat_command<'a>(command: &'a str, arg: &'a str) -> ChatCommand<'a> {
    let builtin = match command {
        "mute" => BuiltinCommand::Mute(arg.parse().ok()),
        "unmute" => BuiltinCommand::Unmute(arg.parse().ok()),
        "mutechat" => BuiltinCommand::MuteChat,
        "unmutechat" => BuiltinCommand::UnmuteChat,
        "kick" => BuiltinCommand::Kick(arg.parse().ok()),
        "kickall" => BuiltinCommand::KickAll(arg),
        "ban" => BuiltinCommand::Ban(arg.parse().ok()),
        "banall" => BuiltinCommand::BanAll(arg),
        "clearbans" => BuiltinCommand::ClearBans,
        "reset" | "resetgame" => BuiltinCommand::Reset,
        "replay" | "record" => BuiltinCommand::SetRecording(arg),
        "lefty" => BuiltinCommand::Lefty,
        "righty" => BuiltinCommand::Righty,
        "admin" => BuiltinCommand::Admin(arg),
        "serverrestart" => BuiltinCommand::RestartServer,
        "list" => BuiltinCommand::List(if arg.is_empty() {
            Some(0)
        } else {
            arg.parse().ok()
        }),
        "search" => BuiltinCommand::Search(arg),
        "ping" => BuiltinCommand::Ping(arg.parse().ok()),
        "pings" => BuiltinCommand::PingByName(arg),
        "view" => BuiltinCommand::View(arg.parse().ok()),
        "views" => BuiltinCommand::ViewByName(arg),
        "restoreview" => BuiltinCommand::RestoreView,
        "t" => BuiltinCommand::TeamChat(arg),
        "version" => BuiltinCommand::Version,
        "git" => BuiltinCommand::Git,
        _ => {
            return ChatCommand::GameMode { command, arg };
        }
    };
    ChatCommand::Builtin(builtin)
}

const TICKS_PER_SECOND: f64 = 100.0;

const TICK_PERIOD: Duration = Duration::from_millis(10);
/// How many ticks are made up at most when the server or the tick thread falls behind.
const MAX_CATCH_UP_TICKS: u32 = 5;

/// The deadlines of the ticks, [TICK_PERIOD] apart, so the tick rate averages 100 per second even
/// when a tick comes late. After a stall of more than [MAX_CATCH_UP_TICKS] ticks the deadlines
/// restart from the end of the stall.
struct TickDeadlines {
    next: Instant,
}

impl TickDeadlines {
    fn new(now: Instant) -> Self {
        Self {
            next: now + TICK_PERIOD,
        }
    }

    /// Moves on to the deadline after the one that was reached at `now`.
    fn advance(&mut self, now: Instant) {
        if now.saturating_duration_since(self.next) > TICK_PERIOD * MAX_CATCH_UP_TICKS {
            self.next = now;
        }
        self.next += TICK_PERIOD;
    }
}

/// Starts a thread that sends a tick to the returned receiver at each tick deadline. The thread
/// sleeps with [std::thread::sleep], which uses high-resolution timers, while tokio's timers
/// only wake up on whole milliseconds, and on Windows often only every 15.6 ms. Up to
/// [MAX_CATCH_UP_TICKS] ticks wait in the channel when the server falls behind, and the rest are
/// dropped. The thread stops when the receiver is dropped.
fn spawn_tick_thread() -> std::io::Result<tokio::sync::mpsc::Receiver<()>> {
    let (sender, receiver) = tokio::sync::mpsc::channel(MAX_CATCH_UP_TICKS as usize);
    std::thread::Builder::new()
        .name("tick".to_owned())
        .spawn(move || {
            let mut deadlines = TickDeadlines::new(Instant::now());
            loop {
                std::thread::sleep(deadlines.next.saturating_duration_since(Instant::now()));
                if let Err(TrySendError::Closed(())) = sender.try_send(()) {
                    return;
                }
                deadlines.advance(Instant::now());
            }
        })?;
    Ok(receiver)
}

/// Measures how often the tick timer fires, to catch a server that runs slower than the game
/// clock assumes.
struct TickRateMeter {
    window_start: Instant,
    ticks: u32,
    reported_once: bool,
}

impl TickRateMeter {
    const WINDOW: Duration = Duration::from_secs(60);

    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            ticks: 0,
            reported_once: false,
        }
    }

    /// Counts a tick, and returns the measured rate in ticks per second when a window is done.
    fn tick(&mut self, now: Instant) -> Option<f64> {
        self.ticks += 1;
        let elapsed = now - self.window_start;
        if elapsed < Self::WINDOW {
            return None;
        }
        let rate = f64::from(self.ticks) / elapsed.as_secs_f64();
        self.window_start = now;
        self.ticks = 0;
        Some(rate)
    }

    fn tick_and_report(&mut self) {
        let Some(rate) = self.tick(Instant::now()) else {
            return;
        };
        if rate < TICKS_PER_SECOND * 0.99 {
            warn!(
                "Tick rate is {rate:.2} ticks/s, expected {TICKS_PER_SECOND}; game time runs slow"
            );
        } else if !self.reported_once {
            info!("Tick rate is {rate:.2} ticks/s");
        }
        self.reported_once = true;
    }
}

async fn send_updates(
    game_id: u32,
    history: &PacketHistory,
    game_step: u32,
    scoreboard: &ScoreboardValues,
    players: &PlayerSlots,
    socket: &mut HQMFramedSocket,
    force_view: Option<PlayerIndex>,
) {
    let update = HQMServerUpdate {
        game_id,
        history,
        game_step,
        scoreboard,
        force_view,
    };

    for (_, player) in players.iter_players() {
        if let ServerPlayerData::NetworkPlayer { data } = &player.data {
            let message = HQMServerToClientMessage::Update {
                update,
                recipient: data,
            };
            let _ = socket.send((message, data.addr)).await;
        }
    }
}

/// Starts an HQM server. This method will not return until the server has terminated.
pub async fn run_server<B: GameMode>(
    port: u16,
    public: Option<&str>,
    config: ServerConfiguration,
    physics_config: PhysicsConfiguration,
    ban: Box<dyn BanCheck>,
    recording: Box<dyn RecordingSaveMethod>,
    mut behaviour: B,
) -> std::io::Result<()> {
    let reqwest_client = reqwest::Client::new();

    let mut server = HQMServer::new(config, physics_config, ban, recording);
    info!("Server started");

    behaviour.init(&mut server.context());
    server.start_requested_game(&mut behaviour);

    let mut ticks = spawn_tick_thread()?;

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let socket = Arc::new(tokio::net::UdpSocket::bind(&addr).await?);
    info!(
        "Server listening at address {:?}",
        socket.local_addr().unwrap()
    );

    async fn get_http_response(
        client: &reqwest::Client,
        address: &str,
    ) -> Result<SocketAddr, Box<dyn Error + Send + Sync>> {
        let response = client.get(address).send().await?.text().await?;

        let split = response.split_ascii_whitespace().collect::<Vec<&str>>();

        let addr = split.get(1).unwrap_or(&"").parse::<IpAddr>()?;
        let port = split.get(2).unwrap_or(&"").parse::<u16>()?;
        Ok(SocketAddr::new(addr, port))
    }

    if let Some(public) = public {
        let mut socket = UdpFramed::new(socket.clone(), HQMMessageCodec);
        let reqwest_client = reqwest_client.clone();
        let address = public.to_string();
        tokio::spawn(async move {
            loop {
                let master_server = get_http_response(&reqwest_client, &address).await;
                match master_server {
                    Ok(addr) => {
                        for _ in 0..60 {
                            let res = socket.send((HQMMasterServerRegistration, addr)).await;
                            if res.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(e);
                        tokio::time::sleep(Duration::from_secs(15)).await;
                    }
                }
            }
        });
    }
    let mut socket = UdpFramed::new(socket, HQMMessageCodec);
    let mut tick_rate = TickRateMeter::new(Instant::now());
    loop {
        tokio::select! {
            tick = ticks.recv() => {
                if tick.is_none() {
                    return Err(std::io::Error::other("the tick thread stopped"));
                }
                tick_rate.tick_and_report();
                server.tick(&mut socket, &mut behaviour).await
            },
            message = socket.next() => {
                let Some(Ok((data, addr))) = message else {
                    continue;
                };
                server
                    .handle_message(addr, &mut socket, data, &mut behaviour)
                    .await;
            }
        }
    }
}

/// A server with default settings and recording turned off, for tests.
#[cfg(test)]
pub(crate) fn test_server() -> HQMServer {
    HQMServer::new(
        ServerConfiguration {
            welcome: vec![],
            password: None,
            player_max: 16,
            recording_enabled: ReplayRecording::Off,
            server_name: "test".to_owned(),
            server_service: None,
        },
        PhysicsConfiguration::default(),
        Box::new(ban::InMemoryBanCheck::new()),
        Box::new(recording::RecordingSaveToFile::new(".".into())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{RulesState, SkaterHand};
    use crate::gamemode::standard_match::test_game::{MatchGame, ice_the_puck, match_server};
    use crate::gamemode::standard_match::{
        IcingConfiguration, MatchConfiguration, StandardMatchGameMode,
    };
    use crate::gamemode::util::SpawnPoint;
    use crate::gamemode::warmup::PermanentWarmup;
    use crate::players::ServerPlayerData;
    use tokio_util::codec::Encoder;

    use crate::game::PhysicsEvent;
    use crate::gamemode::{BotLifetime, ServerPlayerType};
    use crate::players::{MAX_PLAYERS, ServerPlayer};
    use crate::server::test_server::{EmptyGameMode, Received, TestServer, chats, received};

    fn network_player() -> ServerPlayer {
        ServerPlayer::new_network_player(
            PlayerIndex(0),
            "test",
            "127.0.0.1:12345".parse().unwrap(),
            &[],
        )
    }

    fn encode_server_message(message: HQMServerToClientMessage<'_>, buf: &mut BytesMut) {
        let mut codec = HQMMessageCodec;
        assert!(codec.encode(message, buf).is_ok());
    }

    #[test]
    fn parses_builtin_commands_without_forwarding_them_to_the_game_mode() {
        assert_eq!(
            parse_chat_command("kick", "12"),
            ChatCommand::Builtin(BuiltinCommand::Kick(Some(PlayerIndex(12))))
        );
        assert_eq!(
            parse_chat_command("record", "standby"),
            ChatCommand::Builtin(BuiltinCommand::SetRecording("standby"))
        );
        assert_eq!(
            parse_chat_command("list", ""),
            ChatCommand::Builtin(BuiltinCommand::List(Some(0)))
        );
        assert_eq!(
            parse_chat_command("view", "not-a-player"),
            ChatCommand::Builtin(BuiltinCommand::View(None))
        );
        assert_eq!(
            parse_chat_command("resetgame", ""),
            ChatCommand::Builtin(BuiltinCommand::Reset)
        );
    }

    #[test]
    fn preserves_unknown_commands_for_the_game_mode() {
        match parse_chat_command("faceoff", "center") {
            ChatCommand::GameMode { command, arg } => {
                assert_eq!(command, "faceoff");
                assert_eq!(arg, "center");
            }
            ChatCommand::Builtin(_) => panic!("unknown command was treated as built-in"),
        }
    }

    #[test]
    fn encodes_new_game_packet_without_replication_data() {
        let mut player = network_player();
        let ServerPlayerData::NetworkPlayer { data } = &mut player.data else {
            panic!("expected a network player");
        };
        let mut buf = BytesMut::new();
        let history = PacketHistory::new();
        let scoreboard = ScoreboardValues::default();

        encode_server_message(
            HQMServerToClientMessage::Update {
                update: HQMServerUpdate {
                    game_id: 42,
                    history: &history,
                    game_step: 0,
                    scoreboard: &scoreboard,
                    force_view: None,
                },
                recipient: data,
            },
            &mut buf,
        );

        assert_eq!(buf.as_ref(), b"Hock\x06\x2a\x00\x00\x00");
    }

    #[test]
    fn encodes_update_packet_when_the_client_is_in_the_current_game() {
        let mut player = network_player();
        let ServerPlayerData::NetworkPlayer { data } = &mut player.data else {
            panic!("expected a network player");
        };
        data.game_id = 42;

        let mut history = PacketHistory::new();
        history.push([const { ObjectPacket::None }; 32]);
        let scoreboard = ScoreboardValues::default();
        let mut buf = BytesMut::new();

        encode_server_message(
            HQMServerToClientMessage::Update {
                update: HQMServerUpdate {
                    game_id: 42,
                    history: &history,
                    game_step: 11,
                    scoreboard: &scoreboard,
                    force_view: None,
                },
                recipient: data,
            },
            &mut buf,
        );

        assert_eq!(&buf[..9], b"Hock\x05\x2a\x00\x00\x00");
        assert!(buf.len() > 9);
    }

    #[test]
    fn packet_history_returns_only_retained_sequences() {
        let mut history = PacketHistory::new();
        let first = history.push([const { ObjectPacket::None }; 32]);
        for _ in 0..PACKET_HISTORY_LEN {
            history.push([const { ObjectPacket::None }; 32]);
        }

        assert!(history.objects_for(first).is_none());
        assert!(history.objects_for(history.latest_sequence()).is_some());
    }

    fn tick_deadlines(start: Instant, reached_at_ms: &[u64]) -> Vec<u128> {
        let mut deadlines = TickDeadlines::new(start);
        reached_at_ms
            .iter()
            .map(|&ms| {
                deadlines.advance(start + Duration::from_millis(ms));
                (deadlines.next - start).as_millis()
            })
            .collect()
    }

    #[test]
    fn tick_deadlines_are_10_ms_apart() {
        let start = Instant::now();
        assert_eq!(tick_deadlines(start, &[10, 20, 30]), [20, 30, 40]);
    }

    #[test]
    fn tick_deadlines_stay_on_schedule_after_a_short_delay() {
        let start = Instant::now();
        // The ticks due at 20 and 30 ms come at 35 ms, and the next one is due on time at 40 ms
        assert_eq!(tick_deadlines(start, &[10, 35, 35]), [20, 30, 40]);
    }

    #[test]
    fn tick_deadlines_restart_after_a_long_stall() {
        let start = Instant::now();
        assert_eq!(tick_deadlines(start, &[10, 210, 220]), [20, 220, 230]);
    }

    #[test]
    fn tick_thread_keeps_a_limited_number_of_ticks_waiting() {
        let mut ticks = spawn_tick_thread().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let waiting = std::iter::from_fn(|| ticks.try_recv().ok()).count();
        assert_eq!(waiting, MAX_CATCH_UP_TICKS as usize);
    }

    #[test]
    fn tick_rate_meter_reports_the_rate_of_each_window() {
        let start = Instant::now();
        let mut meter = TickRateMeter::new(start);
        let tick_at = |ms: u64| start + Duration::from_millis(ms);

        // 100 ticks per second for the first window
        for i in 1..6000 {
            assert_eq!(meter.tick(tick_at(i * 10)), None);
        }
        assert_eq!(meter.tick(tick_at(60_000)), Some(100.0));

        // 64 ticks per second for the next window
        let ticks: Vec<_> = (1..=3840)
            .map(|i| meter.tick(tick_at(60_000 + i * 15_625 / 1000)))
            .collect();
        assert!(ticks[..3839].iter().all(Option::is_none));
        let rate = ticks[3839].unwrap();
        assert!((rate - 64.0).abs() < 0.01, "{rate}");
    }

    #[test]
    fn packet_numbers_skip_the_no_packet_sentinel_when_wrapping() {
        assert_eq!(PacketNumber(u32::MAX - 1).next(), PacketNumber(0));
        assert_eq!(PacketNumber::from_wire(u32::MAX), None);
    }

    #[test]
    fn replay_queue_ignores_expired_ticks() {
        let mut history = HQMTickHistory::new();
        history.set_history_capacity(2);
        for _ in 0..3 {
            history.advance_step();
            history.record_tick(
                [const { ObjectPacket::None }; 32],
                std::array::from_fn(|_| None),
            );
        }

        assert_eq!(history.add_replay_to_queue(0, 3, None), 2);
        let queued_steps: Vec<_> = std::iter::from_fn(|| history.pop_replay_tick())
            .map(|(_, tick)| tick.game_step)
            .collect();

        assert_eq!(queued_steps, vec![1, 2]);
        assert_eq!(history.add_replay_to_queue(2, 1, None), 0);
        assert!(!history.is_in_replay());
    }

    #[test]
    fn skater_assignment_and_object_slot_are_updated_together() {
        let mut state = HQMServerState::new();
        let player_id = state
            .player_message_state
            .add_player("test", "127.0.0.1:12345".parse().unwrap())
            .unwrap();

        assert!(state.skater_links().spawn_skater(
            player_id,
            Team::Red,
            Vec3::ZERO,
            Rot3::IDENTITY,
            false
        ));
        let object_slot = state
            .player_message_state
            .players
            .get_player(player_id)
            .and_then(|player| player.skater_assignment())
            .map(|(slot, _)| slot)
            .unwrap();
        assert!(matches!(
            state.objects[object_slot.index()],
            Some(GameObject::Skater(id, _)) if id == player_id
        ));

        assert!(state.skater_links().move_to_spectator(player_id));
        assert!(
            state
                .player_message_state
                .players
                .get_player(player_id)
                .is_some_and(|player| !player.has_skater())
        );
        assert!(state.objects[object_slot.index()].is_none());
    }

    #[test]
    fn after_tick_mutations_are_captured_in_current_packet_history() {
        struct SpawnsPuckAfterTick;

        impl GameMode for SpawnsPuckAfterTick {
            fn before_tick(&mut self, _ctx: &mut GameContext) {}

            fn after_tick(
                &mut self,
                ctx: &mut GameContext,
                _events: &[PhysicsEvent],
            ) -> ScoreboardValues {
                ctx.objects
                    .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));
                ScoreboardValues::default()
            }
        }

        let mut server = test_server();
        let mut behaviour = SpawnsPuckAfterTick;
        server.game_step(&mut behaviour);

        assert!(matches!(
            server.state.replication.history.current_objects()[0],
            ObjectPacket::Puck(_)
        ));
    }

    #[test]
    fn context_new_game_resets_state_once_requested_and_keeps_later_messages() {
        let mut server = test_server();
        let player_id = server
            .state
            .player_message_state
            .add_player("test", "127.0.0.1:12345".parse().unwrap())
            .unwrap();
        server.has_current_game_been_active = true;
        let game_id = server.game_id;

        let mut ctx = server.context();
        ctx.objects
            .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));
        assert!(ctx.spawn_skater(player_id, Team::Red, Vec3::X, Rot3::IDENTITY, false));
        ctx.messages.add_server_chat_message("before");
        ctx.new_game();
        ctx.messages.add_server_chat_message("after");

        // The current game goes on until the server carries out the request.
        assert_eq!(server.game_id, game_id);
        assert!(server.state.objects.iter().any(Option::is_some));

        server.start_requested_game(&mut EmptyGameMode);

        assert_eq!(server.game_id, game_id + 1);
        // A player is on the server, so the new game has started right away.
        assert!(server.has_current_game_been_active);
        assert!(server.state.objects.iter().all(Option::is_none));
        server.state.player_message_state.flush_messages();
        let player = server
            .state
            .player_message_state
            .players
            .get_player(player_id)
            .unwrap();
        assert!(!player.has_skater());
        let players = &server.state.player_message_state.players;
        assert_eq!(chats(received(players, player_id)), ["after"]);
    }

    #[test]
    fn all_message_kinds_keep_their_order_per_player() {
        let mut state = HQMServerState::new();
        let red = state
            .player_message_state
            .add_player("red", "127.0.0.1:1".parse().unwrap())
            .unwrap();
        let blue = state
            .player_message_state
            .add_player("blue", "127.0.0.1:2".parse().unwrap())
            .unwrap();
        let spectator = state
            .player_message_state
            .add_player("spectator", "127.0.0.1:3".parse().unwrap())
            .unwrap();
        state.player_message_state.flush_messages();
        let start = [red, blue].map(|id| received(&state.player_message_state.players, id).len());

        let links = &mut state.skater_links();
        assert!(links.spawn_skater(red, Team::Red, Vec3::ZERO, Rot3::IDENTITY, false));
        links
            .messages
            .add_goal_message(links.players, Team::Red, Some(red), None);
        links.messages.add_server_chat_message("a");
        links.messages.add_directed_server_chat_message("b", red);
        state.player_message_state.add_user_team_message("c", red);
        state.player_message_state.add_server_chat_message("d");
        assert!(state.skater_links().remove_player(spectator, true));
        state.player_message_state.flush_messages();

        let messages_since = |player_id, start: usize| {
            received(&state.player_message_state.players, player_id).split_off(start)
        };
        let red_update = |name: &str| Received::PlayerUpdate {
            index: red.index,
            name: name.to_owned(),
            team: Some(Team::Red),
        };
        let goal = Received::Goal {
            team: Team::Red,
            scorer: Some(red.index),
            assist: None,
        };
        let chat = |from: Option<PlayerId>, text: &str| Received::Chat {
            from: from.map(|player_id| player_id.index),
            text: text.to_owned(),
        };
        let removed = Received::PlayerRemoved {
            index: spectator.index,
        };
        assert_eq!(
            messages_since(red, start[0]),
            [
                red_update("red"),
                goal.clone(),
                chat(None, "a"),
                chat(None, "b"),
                red_update("[Red] red"),
                chat(Some(red), "c"),
                red_update("red"),
                chat(None, "d"),
                removed.clone(),
            ]
        );
        assert_eq!(
            messages_since(blue, start[1]),
            [
                red_update("red"),
                goal,
                chat(None, "a"),
                chat(None, "d"),
                removed,
            ]
        );
    }

    #[test]
    fn game_abandoned_is_called_before_the_game_is_reset() {
        #[derive(Default)]
        struct TracksLifecycle {
            started: u32,
            abandoned_with_pucks: Vec<usize>,
        }

        impl GameMode for TracksLifecycle {
            fn before_tick(&mut self, _ctx: &mut GameContext) {}

            fn after_tick(
                &mut self,
                _ctx: &mut GameContext,
                _events: &[PhysicsEvent],
            ) -> ScoreboardValues {
                ScoreboardValues::default()
            }

            fn game_started(&mut self, ctx: &mut GameContext) {
                self.started += 1;
                ctx.objects
                    .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));
            }

            fn game_abandoned(&mut self, ctx: &mut GameContext) {
                self.abandoned_with_pucks.push(ctx.objects.pucks().count());
            }
        }

        let address = "127.0.0.1:12345".parse().unwrap();
        let mut server = test_server();
        let mut behaviour = TracksLifecycle::default();

        let player_id = server
            .state
            .player_message_state
            .add_player("test", address)
            .unwrap();
        server.advance_tick(&mut behaviour);
        assert_eq!(behaviour.started, 1);

        server.state.remove_player(player_id, true);
        server.advance_tick(&mut behaviour);
        server.advance_tick(&mut behaviour);
        assert_eq!(behaviour.abandoned_with_pucks, [1]);
        assert!(server.state.objects.iter().all(Option::is_none));

        server
            .state
            .player_message_state
            .add_player("test", address)
            .unwrap();
        server.advance_tick(&mut behaviour);
        assert_eq!(behaviour.started, 2);
    }

    #[test]
    fn bots_are_controlled_by_the_game_mode_and_do_not_keep_a_game_going() {
        #[derive(Default)]
        struct DrivesBots {
            skating: Option<PlayerId>,
            standing: Option<PlayerId>,
            abandoned: u32,
        }

        impl GameMode for DrivesBots {
            fn before_tick(&mut self, ctx: &mut GameContext) {
                let skating = self.skating.unwrap();
                ctx.players.get_mut(skating).unwrap().input_mut().fwbw = 1.0;
            }

            fn after_tick(
                &mut self,
                _ctx: &mut GameContext,
                _events: &[PhysicsEvent],
            ) -> ScoreboardValues {
                ScoreboardValues::default()
            }

            fn game_started(&mut self, ctx: &mut GameContext) {
                let skating = ctx.add_bot("skating", BotLifetime::Game).unwrap();
                let standing = ctx.add_bot("standing", BotLifetime::Game).unwrap();
                let rot = Rot3::IDENTITY;
                assert!(ctx.spawn_skater(
                    skating,
                    Team::Red,
                    Vec3::new(10.0, 1.5, 30.0),
                    rot,
                    false
                ));
                assert!(ctx.spawn_skater(
                    standing,
                    Team::Red,
                    Vec3::new(20.0, 1.5, 30.0),
                    rot,
                    false
                ));
                self.skating = Some(skating);
                self.standing = Some(standing);
            }

            fn game_abandoned(&mut self, ctx: &mut GameContext) {
                self.abandoned += 1;
                // The bots are still here, and leave when the game is reset.
                assert!(ctx.players.get(self.skating.take().unwrap()).is_some());
                assert!(ctx.players.get(self.standing.take().unwrap()).is_some());
            }
        }

        fn distance_moved(ctx: &GameContext, bot: PlayerId, start: Vec3) -> f32 {
            let pos = ctx.objects.skater(bot).unwrap().body.pos;
            (pos - start).with_y(0.0).length()
        }

        let mut t = TestServer::new(DrivesBots::default());
        let player = t.join("test");
        t.ticks(100);

        let skating = t.mode.skating.unwrap();
        let standing = t.mode.standing.unwrap();
        let ctx = t.ctx();
        for bot in [skating, standing] {
            let bot = ctx.players.get(bot).unwrap();
            assert!(bot.player_type() == ServerPlayerType::Bot);
            assert_eq!(bot.team(), Some(Team::Red));
        }
        let skated = distance_moved(&ctx, skating, Vec3::new(10.0, 1.5, 30.0));
        let stood = distance_moved(&ctx, standing, Vec3::new(20.0, 1.5, 30.0));
        assert!(
            skated > 0.1 && skated > 10.0 * stood,
            "skated {skated} m, stood {stood} m"
        );
        assert!(t.take_messages(player).contains(&Received::PlayerUpdate {
            index: skating.index,
            name: "skating".to_owned(),
            team: Some(Team::Red),
        }));

        t.leave(player);
        t.tick();
        assert_eq!(t.mode.abandoned, 1);
        assert_eq!(t.ctx().players.iter().count(), 0);
        assert!(t.server.state.objects.iter().all(Option::is_none));
    }

    #[test]
    fn add_bot_returns_none_when_all_player_slots_are_taken() {
        let mut t = TestServer::new(EmptyGameMode);
        let mut ctx = t.ctx();
        let bots: Vec<_> = (0..MAX_PLAYERS)
            .map(|i| {
                ctx.add_bot(&format!("bot {i}"), BotLifetime::Server)
                    .unwrap()
            })
            .collect();
        assert!(ctx.add_bot("one too many", BotLifetime::Server).is_none());

        assert!(ctx.remove_player(bots[3]));
        let replacement = ctx.add_bot("replacement", BotLifetime::Server).unwrap();
        assert_eq!(replacement.index, bots[3].index);
        assert!(ctx.players.get(bots[3]).is_none());
    }

    #[test]
    fn new_game_removes_game_bots_and_keeps_server_bots() {
        let mut t = TestServer::new(EmptyGameMode);
        let player = t.join("player");
        t.tick();
        let mut ctx = t.ctx();
        let game_bot = ctx.add_bot("game bot", BotLifetime::Game).unwrap();
        let server_bot = ctx.add_bot("server bot", BotLifetime::Server).unwrap();

        ctx.new_game();
        t.server.start_requested_game(&mut t.mode);

        let ctx = t.ctx();
        assert!(ctx.players.get(game_bot).is_none());
        assert!(ctx.players.get(server_bot).is_some());
        let messages = t.take_messages(player);
        assert!(messages.contains(&Received::PlayerRemoved {
            index: game_bot.index
        }));
        assert!(messages.contains(&Received::PlayerUpdate {
            index: server_bot.index,
            name: "server bot".to_owned(),
            team: None,
        }));
    }

    #[test]
    fn a_requested_new_game_starts_once_the_event_is_handled() {
        #[derive(Default)]
        struct RestartsOnRequest {
            events: Vec<String>,
        }

        impl GameMode for RestartsOnRequest {
            fn before_tick(&mut self, _ctx: &mut GameContext) {}

            fn after_tick(
                &mut self,
                _ctx: &mut GameContext,
                _events: &[PhysicsEvent],
            ) -> ScoreboardValues {
                ScoreboardValues::default()
            }

            fn handle_command(
                &mut self,
                ctx: &mut GameContext,
                _cmd: &str,
                _arg: &str,
                _player_id: PlayerId,
            ) -> CommandResult {
                ctx.new_game();
                // The current game is still there for the rest of the hook.
                let pucks = ctx.objects.pucks().count();
                self.events.push(format!("restart with {pucks} puck"));
                ctx.messages.add_server_chat_message("Restarting");
                CommandResult::Handled
            }

            fn game_started(&mut self, ctx: &mut GameContext) {
                self.events.push("started".to_owned());
                ctx.objects
                    .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));
            }

            fn game_abandoned(&mut self, _ctx: &mut GameContext) {
                self.events.push("abandoned".to_owned());
            }

            fn before_player_exit(
                &mut self,
                ctx: &mut GameContext,
                _player_id: PlayerId,
                _reason: ExitReason,
            ) {
                ctx.new_game();
            }
        }

        let mut t = TestServer::new(RestartsOnRequest::default());
        let player = t.join("player");
        t.tick();
        assert_eq!(t.mode.events, ["started"]);
        t.take_messages(player);

        t.chat(player, "/restart");
        assert_eq!(t.mode.events, ["started", "restart with 1 puck", "started"]);
        assert_eq!(t.ctx().objects.pucks().count(), 1);
        assert_eq!(t.take_chats(player), ["Restarting"]);

        // The last player leaving asks for a new game, which waits for a player to join, and
        // the game it ended is not abandoned.
        t.leave(player);
        t.tick();
        assert_eq!(t.ctx().objects.pucks().count(), 0);
        assert_eq!(t.mode.events.len(), 3);
        t.join("next");
        t.tick();
        assert_eq!(t.mode.events.last().unwrap(), "started");
        assert!(!t.mode.events.contains(&"abandoned".to_owned()));
    }

    #[test]
    fn bots_can_chat_to_everyone_and_to_their_team() {
        let mut t = TestServer::new(EmptyGameMode);
        let red = t.join("red");
        let blue = t.join("blue");
        let mut ctx = t.ctx();
        let bot = ctx.add_bot("bot", BotLifetime::Server).unwrap();
        let benched_bot = ctx.add_bot("benched", BotLifetime::Server).unwrap();
        for (player, team) in [(red, Team::Red), (blue, Team::Blue), (bot, Team::Red)] {
            assert!(ctx.spawn_skater(player, team, Vec3::ZERO, Rot3::IDENTITY, false));
        }
        t.take_messages(red);
        t.take_messages(blue);

        let mut ctx = t.ctx();
        ctx.add_user_chat_message("hello everyone", bot);
        ctx.add_user_team_message("hello red", bot);
        ctx.add_user_team_message("not on the ice", benched_bot);

        let chat = |text: &str| Received::Chat {
            from: Some(bot.index),
            text: text.to_owned(),
        };
        let update = |name: &str| Received::PlayerUpdate {
            index: bot.index,
            name: name.to_owned(),
            team: Some(Team::Red),
        };
        assert_eq!(
            t.take_messages(red),
            [
                chat("hello everyone"),
                update("[Red] bot"),
                chat("hello red"),
                update("bot"),
            ]
        );
        assert_eq!(t.take_messages(blue), [chat("hello everyone")]);
    }

    #[test]
    fn unknown_game_mode_commands_get_a_reply_to_the_sender() {
        #[derive(Default)]
        struct KnowsOneCommand {
            received: Vec<String>,
        }

        impl GameMode for KnowsOneCommand {
            fn before_tick(&mut self, _ctx: &mut GameContext) {}

            fn after_tick(
                &mut self,
                _ctx: &mut GameContext,
                _events: &[PhysicsEvent],
            ) -> ScoreboardValues {
                ScoreboardValues::default()
            }

            fn handle_command(
                &mut self,
                _ctx: &mut GameContext,
                cmd: &str,
                arg: &str,
                _player_id: PlayerId,
            ) -> CommandResult {
                self.received.push(format!("{cmd}({arg})"));
                if cmd == "known" {
                    CommandResult::Handled
                } else {
                    CommandResult::Unknown
                }
            }
        }

        let mut t = TestServer::new(KnowsOneCommand::default());
        let player = t.join("player");
        let other = t.join("other");
        t.take_messages(player);
        t.take_messages(other);

        t.chat(player, "/known with args");
        t.chat(player, "/version");
        t.chat(player, "/typo");

        assert_eq!(t.mode.received, ["known(with args)", "typo()"]);
        let chats = t.take_chats(player);
        assert_eq!(chats.len(), 2, "{chats:?}");
        assert!(chats[0].starts_with("Migo HQM Server, version"));
        assert_eq!(chats[1], "Unknown command /typo");
        assert_eq!(t.take_chats(other), Vec::<String>::new());
    }

    #[test]
    fn welcome_messages_are_sent_to_joining_players() {
        let mut t = TestServer::new(StandardMatchGameMode::new(
            MatchConfiguration::default(),
            5,
            SpawnPoint::Center,
        ));
        t.server.config.welcome = vec!["Welcome!".to_owned(), "Have fun".to_owned()];
        let player = t.join("player");
        let chats = t.take_chats(player);
        assert_eq!(chats[..2], ["Welcome!", "Have fun"]);
    }

    #[test]
    fn player_max_limits_the_players_on_the_server() {
        let mut t = TestServer::new(PermanentWarmup::new(1, SpawnPoint::Center));
        t.server.config.player_max = 2;
        t.join("first");
        t.join("second");
        assert!(t.try_join("third").is_none());
    }

    #[test]
    fn team_chat_reaches_only_team_mates() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 2, 1);
        let (red0, red1, blue) = (game.red[0], game.red[1], game.blue[0]);
        game.t.take_messages(red1);
        game.t.take_messages(blue);
        game.chat(red0, "/t go left");
        assert!(game.t.take_chats(red1).contains(&"go left".to_owned()));
        assert!(!game.t.take_chats(blue).contains(&"go left".to_owned()));
    }

    #[test]
    fn list_shows_up_to_five_player_ids_from_an_optional_start() {
        let mut t = match_server();
        let players: Vec<_> = (0..7).map(|i| t.join(&format!("player{i}"))).collect();
        let asker = players[0];
        t.take_messages(asker);
        t.chat(asker, "/list");
        assert_eq!(
            t.take_chats(asker),
            [
                "0: player0",
                "1: player1",
                "2: player2",
                "3: player3",
                "4: player4"
            ]
        );
        t.chat(asker, "/list 5");
        assert_eq!(t.take_chats(asker), ["5: player5", "6: player6"]);
    }

    #[test]
    fn search_lists_players_whose_name_contains_the_text() {
        let mut t = match_server();
        let asker = t.join("asker");
        t.join("MigoMipo");
        t.join("Other");
        t.take_messages(asker);
        t.chat(asker, "/search Mipo");
        assert_eq!(t.take_chats(asker), ["1: MigoMipo"]);
    }

    #[test]
    fn view_enters_first_person_view_of_a_player() {
        let mut t = match_server();
        let viewer = t.join("viewer");
        t.join("target");
        t.take_messages(viewer);
        t.chat(viewer, "/view 1");
        assert_eq!(t.take_chats(viewer), ["You are now viewing target"]);
        t.chat(viewer, "/restoreview");
        assert_eq!(t.take_chats(viewer), ["View has been restored"]);
    }

    #[test]
    fn view_from_the_ice_makes_the_player_a_spectator() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        let viewer = game.red[0];
        game.chat(viewer, "/view 1");
        assert!(game.heard("You are now viewing blue0"), "{:?}", game.heard);
        assert!(game.t.ctx().objects.skater(viewer).is_none());
    }

    #[test]
    fn views_finds_a_player_by_name_or_lists_the_matches() {
        let mut t = match_server();
        let viewer = t.join("viewer");
        t.join("target one");
        t.join("target two");
        t.take_messages(viewer);
        t.chat(viewer, "/views one");
        assert_eq!(t.take_chats(viewer), ["You are now viewing target one"]);
        t.chat(viewer, "/views target");
        assert_eq!(
            t.take_chats(viewer),
            [
                "Multiple matches found, use /view X",
                "1: target one",
                "2: target two"
            ]
        );
    }

    fn add_ping(t: &mut TestServer<impl GameMode>, player: PlayerId, seconds: f32) {
        let player = t
            .server
            .state
            .player_message_state
            .players
            .get_player_mut(player)
            .unwrap();
        let ServerPlayerData::NetworkPlayer { data } = &mut player.data else {
            panic!("expected a network player");
        };
        data.last_ping.push(seconds);
    }

    #[test]
    fn ping_and_pings_show_a_players_server_side_ping() {
        let mut t = match_server();
        let asker = t.join("asker");
        let target = t.join("target");
        add_ping(&mut t, target, 0.05);
        t.take_messages(asker);
        t.chat(asker, "/ping 1");
        let chats = t.take_chats(asker);
        assert_eq!(chats[0], "target ping: avg 50 ms");
        t.chat(asker, "/pings targ");
        assert_eq!(t.take_chats(asker), chats);
    }

    fn hand(game: &MatchGame, player: PlayerId) -> SkaterHand {
        game.server_objects()
            .find_map(|object| match object {
                crate::server::GameObject::Skater(id, skater) if *id == player => Some(skater.hand),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn lefty_during_play_applies_after_play_has_stopped() {
        let config = MatchConfiguration {
            icing: IcingConfiguration::NoTouch,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        let player = game.red[0];
        assert_eq!(hand(&game, player), SkaterHand::Right);
        game.chat(player, "/lefty");
        assert!(
            game.heard("Stick hand will change when play has stopped"),
            "{:?}",
            game.heard
        );
        game.tick();
        assert_eq!(hand(&game, player), SkaterHand::Right);

        // Play stops for icing, and the hand changes during the stoppage.
        ice_the_puck(&mut game);
        game.tick_until(100, |game| game.scoreboard.rules_state == RulesState::Icing);
        assert_eq!(hand(&game, player), SkaterHand::Left);
        game.wait_for_play();
        assert_eq!(hand(&game, player), SkaterHand::Left);
    }

    #[test]
    fn players_who_have_not_been_on_the_ice_can_change_hands_during_play() {
        let mut game = MatchGame::start(MatchGame::quick_config(), 1, 1);
        let late = game.t.join("late");
        game.t.take_chats(late);
        game.chat(late, "/lefty");
        assert_eq!(game.t.take_chats(late), Vec::<String>::new());
        game.t.press_join(late, Team::Red);
        game.ticks(2);
        assert_eq!(hand(&game, late), SkaterHand::Left);
    }

    #[test]
    fn leaving_the_ice_during_play_does_not_allow_changing_hands() {
        let config = MatchConfiguration {
            time_period: 5,
            ..MatchGame::quick_config()
        };
        let mut game = MatchGame::start(config, 1, 1);
        let player = game.red[0];
        game.chat(player, "/view 1");
        assert!(game.t.ctx().objects.skater(player).is_none());
        game.heard.clear();
        game.chat(player, "/lefty");
        assert!(
            game.heard("Stick hand will change when play has stopped"),
            "{:?}",
            game.heard
        );
        game.t.press_join(player, Team::Red);
        game.ticks(2);
        assert_eq!(hand(&game, player), SkaterHand::Right);

        game.tick_until(1000, |game| game.scoreboard.period == 2);
        assert_eq!(hand(&game, player), SkaterHand::Left);
    }

    #[test]
    fn lefty_outside_of_play_applies_at_once() {
        let mut game = MatchGame::join(MatchGame::quick_config(), 1, 0);
        game.ticks(2);
        let player = game.red[0];
        game.chat(player, "/lefty");
        assert_eq!(hand(&game, player), SkaterHand::Left);
        game.chat(player, "/righty");
        assert_eq!(hand(&game, player), SkaterHand::Right);
    }
}
