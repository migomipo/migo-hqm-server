use crate::game::{PlayerId, PlayerIndex, PlayerInput, SkaterHand, Team};
use crate::gamemode::BotLifetime;
use crate::server::{HQMClientVersion, ObjectSlot, PacketNumber};
use arraydeque::ArrayDeque;
use arraydeque::behavior::Wrapping;
use std::borrow::Cow;
use std::net::SocketAddr;
use std::rc::Rc;
use tracing::info;

#[derive(Debug, Clone)]
pub(crate) struct PlayerUpdateData {
    pub player_name: Rc<str>,
    pub object: Option<(ObjectSlot, Team)>,
}

#[derive(Debug, Clone)]
pub(crate) enum HQMMessage {
    PlayerUpdate {
        player_index: PlayerIndex,
        data: Option<PlayerUpdateData>,
    },
    Goal {
        team: Team,
        goal_player_index: Option<PlayerIndex>,
        assist_player_index: Option<PlayerIndex>,
    },
    Chat {
        player_index: Option<PlayerIndex>,
        message: Cow<'static, str>,
    },
}

pub(crate) trait PlayerListExt {
    fn get_player_by_index(&self, player_index: PlayerIndex) -> Option<(PlayerId, &ServerPlayer)>;

    fn get_player(&self, player_id: PlayerId) -> Option<&ServerPlayer>;

    fn get_player_mut_by_index(
        &mut self,
        player_index: PlayerIndex,
    ) -> Option<(PlayerId, &mut ServerPlayer)>;

    fn get_player_mut(&mut self, player_id: PlayerId) -> Option<&mut ServerPlayer>;
    fn iter_players(&self) -> impl Iterator<Item = (PlayerId, &ServerPlayer)>;

    fn iter_players_mut(&mut self) -> impl Iterator<Item = (PlayerId, &mut ServerPlayer)>;

    fn find_player_by_addr(&self, addr: SocketAddr) -> Option<(PlayerId, &ServerPlayer)> {
        self.iter_players().find(|(_, x)| {
            if let ServerPlayerData::NetworkPlayer { data } = &x.data {
                data.addr == addr
            } else {
                false
            }
        })
    }

    fn find_player_by_addr_mut(
        &mut self,
        addr: SocketAddr,
    ) -> Option<(PlayerId, &mut ServerPlayer)> {
        self.iter_players_mut().find(|(_, x)| {
            if let ServerPlayerData::NetworkPlayer { data } = &x.data {
                data.addr == addr
            } else {
                false
            }
        })
    }

    fn find_empty_slot(&self) -> Option<PlayerIndex>;
}

pub(crate) const MAX_PLAYERS: usize = 64;

struct PlayerSlot {
    generation: u32,
    player: Option<ServerPlayer>,
}

/// Fixed-size player storage with generation-checked player identifiers.
pub(crate) struct PlayerSlots {
    slots: [PlayerSlot; MAX_PLAYERS],
}

impl PlayerSlots {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| PlayerSlot {
                generation: 0,
                player: None,
            }),
        }
    }

    fn insert(&mut self, index: PlayerIndex, player: ServerPlayer) -> PlayerId {
        let slot = &mut self.slots[index.0];
        debug_assert!(slot.player.is_none());
        slot.player = Some(player);
        PlayerId {
            index,
            counter: slot.generation,
        }
    }

    pub(crate) fn remove(&mut self, player_id: PlayerId) -> Option<ServerPlayer> {
        let slot = self.slots.get_mut(player_id.index.0)?;
        if slot.generation != player_id.counter {
            return None;
        }

        let player = slot.player.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        Some(player)
    }
}

impl PlayerListExt for PlayerSlots {
    fn get_player_by_index(&self, player_index: PlayerIndex) -> Option<(PlayerId, &ServerPlayer)> {
        self.slots.get(player_index.0).and_then(|slot| {
            slot.player.as_ref().map(|player| {
                (
                    PlayerId {
                        index: player_index,
                        counter: slot.generation,
                    },
                    player,
                )
            })
        })
    }

    fn get_player(&self, player_id: PlayerId) -> Option<&ServerPlayer> {
        self.slots
            .get(player_id.index.0)
            .and_then(|slot| match &slot.player {
                Some(player) if slot.generation == player_id.counter => Some(player),
                _ => None,
            })
    }

    fn get_player_mut_by_index(
        &mut self,
        player_index: PlayerIndex,
    ) -> Option<(PlayerId, &mut ServerPlayer)> {
        self.slots.get_mut(player_index.0).and_then(|slot| {
            slot.player.as_mut().map(|player| {
                (
                    PlayerId {
                        index: player_index,
                        counter: slot.generation,
                    },
                    player,
                )
            })
        })
    }

    fn get_player_mut(&mut self, player_id: PlayerId) -> Option<&mut ServerPlayer> {
        self.slots
            .get_mut(player_id.index.0)
            .and_then(|slot| match &mut slot.player {
                Some(player) if slot.generation == player_id.counter => Some(player),
                _ => None,
            })
    }

    fn iter_players(&self) -> impl Iterator<Item = (PlayerId, &ServerPlayer)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(player_index, slot)| {
                slot.player.as_ref().map(|player| {
                    (
                        PlayerId {
                            index: PlayerIndex(player_index),
                            counter: slot.generation,
                        },
                        player,
                    )
                })
            })
    }

    fn iter_players_mut(&mut self) -> impl Iterator<Item = (PlayerId, &mut ServerPlayer)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(player_index, slot)| {
                slot.player.as_mut().map(|player| {
                    (
                        PlayerId {
                            index: PlayerIndex(player_index),
                            counter: slot.generation,
                        },
                        player,
                    )
                })
            })
    }

    fn find_empty_slot(&self) -> Option<PlayerIndex> {
        self.slots
            .iter()
            .position(|slot| slot.player.is_none())
            .map(|x| PlayerIndex(x))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum MessageRetention {
    Transient,
    ForLateJoiners,
}

#[derive(Clone, Copy)]
pub(crate) enum MessageRecording {
    Exclude,
    Include,
}

#[derive(Clone, Copy)]
enum MessageRecipients {
    All,
    Player(PlayerId),
}

/// Messages waiting to be delivered to the players' message queues, along with the message
/// lists for late joiners and recordings.
///
/// Messages are queued here instead of being added to each player's queue directly, so that
/// sending a message does not require mutable access to the player list. The queue is delivered
/// with [ServerPlayersAndMessages::flush_messages] before anything reads the players' message
/// queues or depends on which players a message was sent to, so clients receive the same
/// messages in the same order as if they had been delivered immediately.
pub(crate) struct ServerMessages {
    pending: Vec<(MessageRecipients, Rc<HQMMessage>)>,
    persistent_messages: Vec<Rc<HQMMessage>>,
    recording_messages: Vec<Rc<HQMMessage>>,
}

impl ServerMessages {
    fn new() -> Self {
        Self {
            pending: vec![],
            persistent_messages: vec![],
            recording_messages: vec![],
        }
    }

    pub(crate) fn add_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        let chat = HQMMessage::Chat {
            player_index: None,
            message: message.into(),
        };
        self.broadcast_message(chat, MessageRetention::Transient, MessageRecording::Include);
    }

    pub(crate) fn add_user_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        sender_index: PlayerIndex,
    ) {
        let chat = HQMMessage::Chat {
            player_index: Some(sender_index),
            message: message.into(),
        };
        self.broadcast_message(chat, MessageRetention::Transient, MessageRecording::Include);
    }

    pub(crate) fn add_directed_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver_id: PlayerId,
        sender_index: Option<PlayerIndex>,
    ) {
        let chat = HQMMessage::Chat {
            player_index: sender_index,
            message: message.into(),
        };
        self.pending
            .push((MessageRecipients::Player(receiver_id), Rc::new(chat)));
    }

    pub(crate) fn add_directed_server_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver_id: PlayerId,
    ) {
        self.add_directed_chat_message(message, receiver_id, None);
    }

    pub(crate) fn broadcast_message(
        &mut self,
        message: HQMMessage,
        retention: MessageRetention,
        recording: MessageRecording,
    ) {
        let rc = Rc::new(message);
        if matches!(recording, MessageRecording::Include) {
            self.recording_messages.push(rc.clone());
        }
        if matches!(retention, MessageRetention::ForLateJoiners) {
            self.persistent_messages.push(rc.clone());
        }
        self.pending.push((MessageRecipients::All, rc));
    }

    /// Delivers all queued messages to the players' message queues.
    pub(crate) fn flush(&mut self, players: &mut PlayerSlots) {
        for (recipients, message) in self.pending.drain(..) {
            match recipients {
                MessageRecipients::All => {
                    for (_, player) in players.iter_players_mut() {
                        player.add_message(message.clone());
                    }
                }
                MessageRecipients::Player(player_id) => {
                    if let Some(player) = players.get_player_mut(player_id) {
                        player.add_message(message);
                    }
                }
            }
        }
    }

    /// Clears all messages and the players' message queues, removes the bots that belong to the
    /// game, and announces the remaining players again.
    pub(crate) fn new_game(&mut self, players: &mut PlayerSlots) {
        self.flush(players);
        self.recording_messages.clear();
        self.persistent_messages.clear();

        let game_bots: Vec<PlayerId> = players
            .iter_players()
            .filter(|(_, player)| {
                matches!(
                    player.data,
                    ServerPlayerData::Bot {
                        lifetime: BotLifetime::Game
                    }
                )
            })
            .map(|(player_id, _)| player_id)
            .collect();
        for player_id in game_bots {
            players.remove(player_id);
            // Clients knew the bot in the previous game, so tell them it is gone.
            self.broadcast_message(
                HQMMessage::PlayerUpdate {
                    player_index: player_id.index,
                    data: None,
                },
                MessageRetention::Transient,
                MessageRecording::Exclude,
            );
        }

        for (player_id, player) in players.iter_players_mut() {
            player.reset(player_id.index);
            self.broadcast_message(
                player.get_update_message(player_id.index),
                MessageRetention::ForLateJoiners,
                MessageRecording::Include,
            );
        }
    }

    /// Adds a bot and announces it to the players. Returns `None` if there is no free player slot.
    pub(crate) fn add_bot(
        &mut self,
        players: &mut PlayerSlots,
        player_name: &str,
        lifetime: BotLifetime,
    ) -> Option<PlayerId> {
        let player_index = players.find_empty_slot()?;
        let new_player = ServerPlayer::new_bot(player_name, lifetime);
        let update = new_player.get_update_message(player_index);
        let player_id = players.insert(player_index, new_player);
        self.broadcast_message(
            update,
            MessageRetention::ForLateJoiners,
            MessageRecording::Include,
        );
        Some(player_id)
    }

    /// Returns the player if the player is admin, otherwise sends a message telling the user to log in first.
    pub(crate) fn check_admin_or_deny<'p>(
        &mut self,
        players: &'p PlayerSlots,
        player_id: PlayerId,
    ) -> Option<&'p ServerPlayer> {
        let player = players.get_player(player_id)?;
        if player.is_admin {
            Some(player)
        } else {
            self.add_directed_server_chat_message(
                "Please log in before using that command",
                player_id,
            );
            None
        }
    }

    /// Announces a goal. Scorer and assist are left out if they are no longer on the server.
    pub(crate) fn add_goal_message(
        &mut self,
        players: &PlayerSlots,
        team: Team,
        goal_player_id: Option<PlayerId>,
        assist_player_id: Option<PlayerId>,
    ) {
        let index_if_present = |id: PlayerId| players.get_player(id).is_some().then_some(id.index);
        let message = HQMMessage::Goal {
            team,
            goal_player_index: goal_player_id.and_then(index_if_present),
            assist_player_index: assist_player_id.and_then(index_if_present),
        };
        self.broadcast_message(
            message,
            MessageRetention::ForLateJoiners,
            MessageRecording::Include,
        );
    }
}

pub(crate) struct ServerPlayersAndMessages {
    pub(crate) players: PlayerSlots,
    pub(crate) messages: ServerMessages,
}

impl ServerPlayersAndMessages {
    pub(crate) fn new() -> Self {
        Self {
            players: PlayerSlots::new(),
            messages: ServerMessages::new(),
        }
    }

    /// Delivers all queued messages to the players' message queues.
    pub(crate) fn flush_messages(&mut self) {
        self.messages.flush(&mut self.players);
    }

    /// Returns the player if the player is admin, otherwise sends a message telling the user to log in first.
    pub(crate) fn check_admin_or_deny(&mut self, player_id: PlayerId) -> Option<&ServerPlayer> {
        self.messages.check_admin_or_deny(&self.players, player_id)
    }

    pub(crate) fn add_user_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        sender_index: PlayerIndex,
    ) {
        self.messages.add_user_chat_message(message, sender_index);
    }

    pub(crate) fn add_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        self.messages.add_server_chat_message(message);
    }

    pub fn add_directed_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver_id: PlayerId,
        sender_index: Option<PlayerIndex>,
    ) {
        self.messages
            .add_directed_chat_message(message, receiver_id, sender_index);
    }

    pub fn add_directed_user_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver_id: PlayerId,
        sender_index: PlayerIndex,
    ) {
        self.add_directed_chat_message(message, receiver_id, Some(sender_index));
    }

    pub fn add_directed_server_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver_id: PlayerId,
    ) {
        self.add_directed_chat_message(message, receiver_id, None);
    }

    pub(crate) fn broadcast_message(
        &mut self,
        message: HQMMessage,
        retention: MessageRetention,
        recording: MessageRecording,
    ) {
        self.messages
            .broadcast_message(message, retention, recording);
    }

    pub(crate) fn add_user_team_message(&mut self, message: &str, sender_id: PlayerId) {
        self.messages
            .add_user_team_message(&mut self.players, message.to_owned(), sender_id);
    }
}

impl ServerMessages {
    /// Sends a chat message from a player to the players on the sender's team, with the sender's
    /// name tagged with the team. Nothing is sent if the sender is not on the ice.
    pub(crate) fn add_user_team_message(
        &mut self,
        players: &mut PlayerSlots,
        message: impl Into<Cow<'static, str>>,
        sender_id: PlayerId,
    ) {
        let message = message.into();
        if let Some(player) = players.get_player(sender_id) {
            let team = if let Some((_, team)) = player.object {
                Some(team)
            } else {
                None
            };
            if let Some(team) = team {
                info!(
                    "{} ({}) to team {}: {}",
                    &player.player_name, sender_id, team, message
                );
                let object = player
                    .object
                    .as_ref()
                    .map(|(object_index, team)| (*object_index, *team));

                let team_tag_name = match team {
                    Team::Red => player.player_name_red.clone(),
                    Team::Blue => player.player_name_blue.clone(),
                };

                let change1 = Rc::new(HQMMessage::PlayerUpdate {
                    player_index: sender_id.index,
                    data: Some(PlayerUpdateData {
                        player_name: team_tag_name,
                        object,
                    }),
                });
                let change2 = Rc::new(HQMMessage::PlayerUpdate {
                    player_index: sender_id.index,
                    data: Some(PlayerUpdateData {
                        player_name: player.player_name.clone(),
                        object,
                    }),
                });
                let chat = Rc::new(HQMMessage::Chat {
                    player_index: Some(sender_id.index),
                    message,
                });

                self.send_to_team(players, team, &[change1, chat, change2]);
            }
        }
    }

    fn send_to_team(&mut self, players: &mut PlayerSlots, team: Team, messages: &[Rc<HQMMessage>]) {
        // Team membership is decided when the message is sent, so earlier messages must be
        // delivered first to keep the order.
        self.flush(players);
        for (_, player) in players.iter_players_mut() {
            if player.team().is_some_and(|player_team| player_team == team) {
                for message in messages {
                    player.add_message(message.clone());
                }
            }
        }
    }
}

impl ServerPlayersAndMessages {
    pub(crate) fn add_player(&mut self, player_name: &str, addr: SocketAddr) -> Option<PlayerId> {
        if self.players.find_player_by_addr(addr).is_some() {
            return None;
        }
        // The new player gets a copy of the persistent messages, so earlier messages must be
        // delivered first or the player would receive them twice.
        self.flush_messages();
        let player_index = self.players.find_empty_slot();
        match player_index {
            Some(player_index) => {
                let new_player = ServerPlayer::new_network_player(
                    player_index,
                    player_name,
                    addr,
                    &self.messages.persistent_messages,
                );
                let update = new_player.get_update_message(player_index);

                let player_id = self.players.insert(player_index, new_player);

                self.broadcast_message(
                    update,
                    MessageRetention::ForLateJoiners,
                    MessageRecording::Include,
                );

                Some(player_id)
            }
            _ => None,
        }
    }

    pub(crate) fn recording_messages_since(&self, message_pos: usize) -> &[Rc<HQMMessage>] {
        &self.messages.recording_messages[message_pos..]
    }

    pub(crate) fn recording_message_count(&self) -> usize {
        self.messages.recording_messages.len()
    }
}

pub(crate) struct NetworkPlayerData {
    pub addr: SocketAddr,
    pub client_version: HQMClientVersion,
    pub(crate) inactivity: u32,
    pub known_packet: Option<PacketNumber>,
    pub known_msgpos: usize,
    pub(crate) chat_rep: Option<u8>,
    pub deltatime: u32,
    pub(crate) last_ping: Box<ArrayDeque<f32, 100, Wrapping>>,
    pub view_player_index: PlayerIndex,
    pub game_id: u32,
    pub messages: Vec<Rc<HQMMessage>>,
}

pub(crate) enum ServerPlayerData {
    NetworkPlayer { data: NetworkPlayerData },
    Bot { lifetime: BotLifetime },
}

pub struct ServerPlayer {
    pub player_name: Rc<str>,
    player_name_red: Rc<str>,
    player_name_blue: Rc<str>,
    object: Option<(ObjectSlot, Team)>,
    pub data: ServerPlayerData,
    pub is_admin: bool,
    pub is_muted: MuteStatus,
    pub preferred_hand: SkaterHand,
    pub input: PlayerInput,
}

impl ServerPlayer {
    pub fn new_network_player(
        player_index: PlayerIndex,
        player_name: &str,
        addr: SocketAddr,
        global_messages: &[Rc<HQMMessage>],
    ) -> Self {
        ServerPlayer {
            player_name: player_name.into(),
            player_name_red: format!("[Red] {player_name}").into(),
            player_name_blue: format!("[Blue] {player_name}").into(),
            object: None,
            data: ServerPlayerData::NetworkPlayer {
                data: NetworkPlayerData {
                    addr,
                    client_version: HQMClientVersion::Vanilla,
                    inactivity: 0,
                    known_packet: None,
                    known_msgpos: 0,
                    chat_rep: None,
                    // store latest deltime client sends you to respond with it
                    deltatime: 0,
                    last_ping: Box::new(ArrayDeque::new()),
                    view_player_index: player_index,
                    game_id: u32::MAX,
                    messages: global_messages.to_vec(),
                },
            },
            is_admin: false,
            input: Default::default(),
            is_muted: MuteStatus::NotMuted,
            preferred_hand: SkaterHand::Right,
        }
    }

    pub fn new_bot(player_name: &str, lifetime: BotLifetime) -> Self {
        ServerPlayer {
            player_name: player_name.into(),
            player_name_red: format!("[Red] {player_name}").into(),
            player_name_blue: format!("[Blue] {player_name}").into(),
            object: None,
            data: ServerPlayerData::Bot { lifetime },
            is_admin: false,
            input: Default::default(),
            is_muted: MuteStatus::NotMuted,
            preferred_hand: SkaterHand::Right,
        }
    }

    fn reset(&mut self, player_index: PlayerIndex) {
        self.object = None;
        if let ServerPlayerData::NetworkPlayer { data } = &mut self.data {
            data.known_msgpos = 0;
            data.known_packet = None;
            data.messages.clear();
            data.view_player_index = player_index;
        }
    }

    pub(crate) fn get_update_message(&self, player_index: PlayerIndex) -> HQMMessage {
        HQMMessage::PlayerUpdate {
            player_index,
            data: Some(PlayerUpdateData {
                player_name: self.player_name.clone(),
                object: self
                    .object
                    .as_ref()
                    .map(|(object_slot, team)| (*object_slot, *team)),
            }),
        }
    }

    fn add_message(&mut self, message: Rc<HQMMessage>) {
        if let ServerPlayerData::NetworkPlayer {
            data: NetworkPlayerData { messages, .. },
        } = &mut self.data
        {
            messages.push(message);
        }
    }

    pub(crate) fn ping_data(&self) -> Option<PingData> {
        match self.data {
            ServerPlayerData::NetworkPlayer {
                data: NetworkPlayerData { ref last_ping, .. },
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
            ServerPlayerData::Bot { .. } => None,
        }
    }

    pub fn team(&self) -> Option<Team> {
        self.object.as_ref().map(|x| x.1)
    }

    pub fn has_skater(&self) -> bool {
        self.object.is_some()
    }

    pub(crate) fn skater_assignment(&self) -> Option<(ObjectSlot, Team)> {
        self.object
    }

    pub(crate) fn set_skater_assignment(&mut self, object_slot: ObjectSlot, team: Team) {
        self.object = Some((object_slot, team));
    }

    pub(crate) fn clear_skater_assignment(&mut self) {
        self.object = None;
    }

    pub(crate) fn set_skater_team(&mut self, team: Team) {
        if let Some((_, current_team)) = &mut self.object {
            *current_team = team;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_server::{Received, chats, received};

    #[test]
    fn reused_player_slot_invalidates_the_previous_player_id() {
        let mut slots = PlayerSlots::new();
        let index = PlayerIndex(0);
        let previous_id = slots.insert(
            index,
            ServerPlayer::new_bot("previous", BotLifetime::Server),
        );

        assert!(slots.remove(previous_id).is_some());

        let current_id = slots.insert(index, ServerPlayer::new_bot("current", BotLifetime::Server));
        assert_ne!(previous_id, current_id);
        assert!(slots.get_player(previous_id).is_none());
        assert!(slots.get_player(current_id).is_some());
    }

    fn chat_messages(state: &ServerPlayersAndMessages, player_id: PlayerId) -> Vec<String> {
        chats(received(&state.players, player_id))
    }

    #[test]
    fn queued_messages_keep_their_order_per_player() {
        let mut state = ServerPlayersAndMessages::new();
        let first = state
            .add_player("first", "127.0.0.1:1".parse().unwrap())
            .unwrap();
        let second = state
            .add_player("second", "127.0.0.1:2".parse().unwrap())
            .unwrap();

        state.add_server_chat_message("a");
        state.add_directed_server_chat_message("b", first);
        state.add_server_chat_message("c");
        state.add_directed_server_chat_message("d", second);
        assert!(chat_messages(&state, first).is_empty());

        state.flush_messages();
        assert_eq!(chat_messages(&state, first), ["a", "b", "c"]);
        assert_eq!(chat_messages(&state, second), ["a", "c", "d"]);
    }

    #[test]
    fn late_joiners_get_queued_persistent_messages_once() {
        let mut state = ServerPlayersAndMessages::new();
        state
            .add_player("first", "127.0.0.1:1".parse().unwrap())
            .unwrap();
        state
            .messages
            .add_goal_message(&state.players, Team::Red, None, None);

        let late = state
            .add_player("late", "127.0.0.1:2".parse().unwrap())
            .unwrap();
        state.flush_messages();
        let goals = received(&state.players, late)
            .into_iter()
            .filter(|message| matches!(message, Received::Goal { .. }))
            .count();
        assert_eq!(goals, 1);
    }

    #[test]
    fn directed_messages_are_not_delivered_to_a_new_player_in_the_same_slot() {
        let mut state = ServerPlayersAndMessages::new();
        let address = "127.0.0.1:1".parse().unwrap();
        let old = state.add_player("old", address).unwrap();
        state.add_directed_server_chat_message("for old", old);
        state.players.remove(old);

        let new = state.add_player("new", address).unwrap();
        assert_eq!(new.index, old.index);
        state.flush_messages();
        assert!(chat_messages(&state, new).is_empty());
    }
}

#[derive(Copy, Clone)]
pub(crate) struct PingData {
    pub min: f32,
    pub max: f32,
    pub avg: f32,
    pub deviation: f32,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum MuteStatus {
    NotMuted,
    ShadowMuted,
    Muted,
}
