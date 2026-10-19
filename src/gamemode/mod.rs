use crate::ServerConfiguration;
use crate::game::{
    PhysicsEvent, PlayerId, PlayerIndex, PlayerInput, Puck, Rink, ScoreboardValues, Team,
};
use crate::players::{
    PlayerListExt, ServerPlayer as InternalServerPlayer, ServerPlayerData, ServerPlayersAndMessages,
};
use crate::server::{GameObject, HQMServer, HQMServerState, HQMTickHistory, ObjectExt};
use glam::Vec3;
use glamx::Rot3;
use reborrow::{Reborrow, ReborrowCopyTraits, ReborrowTraits};
use std::borrow::Cow;
use std::cmp::PartialEq;
use std::rc::Rc;

pub mod shootout;
pub mod standard_match;
pub mod util;
pub mod warmup;

/// Specifies the server game behaviour.
///
/// To create a new game mode, you need to write an object that implements this trait.
pub trait GameMode {
    /// Called when the server starts.
    fn init(&mut self, _server: ServerMut) {}

    /// Called once each tick before the physics simulation is done.
    ///
    /// It is recommended that things like spawning pucks and moving players to and from teams are done before the physics simulation, so it should be done here.
    fn before_tick(&mut self, server: ServerMut);

    /// Called once each tick after the physics simulation is done.
    /// A list of physics events that occurred during the simulation is provided. Most of them have to do with the puck's movement.
    ///
    /// Changes to the server state are included in this tick's replication and recording data, but
    /// do not affect physics until the next tick.
    fn after_tick(&mut self, server: ServerMut, events: &[PhysicsEvent]) -> ScoreboardValues;

    /// Called when a chat message starting with "/" is received from a user. This method is called between ticks and not during, so you can do anything here.
    fn handle_command(
        &mut self,
        _server: ServerMut,
        _cmd: &str,
        _arg: &str,
        _player_index: PlayerId,
    ) {
    }

    fn game_started(&mut self, _server: ServerMut) {}

    /// Called right before a player is removed from the server.
    ///
    /// As the player has not yet been removed, it is possible to get the player object from the server handle.
    fn before_player_exit(
        &mut self,
        _server: ServerMut,
        _player_id: PlayerId,
        _reason: ExitReason,
    ) {
    }

    /// Called right after a new player has joined the server.
    fn after_player_join(&mut self, _server: ServerMut, _player_index: PlayerId) {}

    /// Gets the server team size that will be shown in the server list.
    fn server_list_team_size(&self) -> u32 {
        0
    }

    fn include_tick_in_recording(&self, _server: Server) -> bool {
        false
    }
}

/// A struct containing the individual parts of a [ServerMut].
///
/// This is useful if you want to mutably borrow several properties at once without getting in trouble with the borrow checker.
#[non_exhaustive]
pub struct ServerMutParts<'a> {
    pub state: ServerStateMut<'a>,
    pub rink: &'a mut Rink,
    pub config: &'a mut ServerConfiguration,
}

/// Handle to server.
///
/// This is the object you will mainly use to interact with the server state.
#[derive(ReborrowTraits)]
#[Const(Server)]
pub struct ServerMut<'a> {
    #[reborrow]
    pub(crate) server: &'a mut HQMServer,
}

impl<'a> From<&'a mut HQMServer> for ServerMut<'a> {
    fn from(server: &'a mut HQMServer) -> Self {
        Self { server }
    }
}

impl<'a> ServerMut<'a> {
    /// Splits this handle into mutable handles for the independent server parts.
    ///
    /// Unlike [ServerMut::as_mut_parts], the returned handles retain the full lifetime of this
    /// server handle. This is useful when the original `ServerMut` is no longer needed.
    pub fn into_parts(self) -> ServerMutParts<'a> {
        let server = self.server;
        ServerMutParts {
            state: ServerStateMut {
                state: &mut server.state,
            },
            rink: &mut server.rink,
            config: &mut server.config,
        }
    }

    pub fn as_mut_parts(&mut self) -> ServerMutParts<'_> {
        ServerMutParts {
            state: ServerStateMut {
                state: &mut self.server.state,
            },
            rink: &mut self.server.rink,
            config: &mut self.server.config,
        }
    }

    pub fn new_game(&mut self) {
        self.server.new_game()
    }

    pub fn state_mut(&mut self) -> ServerStateMut<'_> {
        ServerStateMut {
            state: &mut self.server.state,
        }
    }

    pub fn rink_mut(&mut self) -> &mut Rink {
        &mut self.server.rink
    }
    pub fn config_mut(&mut self) -> &mut ServerConfiguration {
        &mut self.server.config
    }

    pub fn state(&self) -> ServerState<'_> {
        ServerState {
            state: &self.server.state,
        }
    }
    pub fn rink(&self) -> &Rink {
        &self.server.rink
    }

    pub fn config(&self) -> &ServerConfiguration {
        &self.server.config
    }

    /// Sends a server chat message to all players.
    pub fn add_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        self.state_mut().add_server_chat_message(message);
    }

    /// Sends a server chat message to a single player.
    pub fn add_directed_server_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver: PlayerId,
    ) {
        self.state_mut()
            .add_directed_server_chat_message(message, receiver);
    }

    /// Returns the player's name if the player is admin, otherwise sends a message telling the user to log in first.
    pub fn require_admin(&mut self, player_id: PlayerId) -> Option<Rc<str>> {
        self.state_mut().require_admin(player_id)
    }
}

/// Immutable handle to server.
#[derive(ReborrowCopyTraits)]
pub struct Server<'a> {
    pub(crate) server: &'a HQMServer,
}

impl<'a> From<&'a HQMServer> for Server<'a> {
    fn from(server: &'a HQMServer) -> Self {
        Self { server }
    }
}

impl<'a> Server<'a> {
    /// Gets an immutable reference to player and puck state.
    pub fn state(&self) -> ServerState<'_> {
        ServerState {
            state: &self.server.state,
        }
    }
    pub fn rink(&self) -> &Rink {
        &self.server.rink
    }

    pub fn config(&self) -> &ServerConfiguration {
        &self.server.config
    }
}

#[derive(ReborrowTraits)]
#[Const(ServerState)]
pub struct ServerStateMut<'a> {
    state: &'a mut HQMServerState,
}

impl<'a> ServerStateMut<'a> {
    /// Gets a mutable reference to player state.
    pub fn players_mut(&mut self) -> ServerPlayersMut<'_> {
        ServerPlayersMut {
            state: &mut self.state.player_message_state,
        }
    }
    pub fn replay_mut(&mut self) -> ServerReplayMut<'_> {
        ServerReplayMut {
            replay: &mut self.state.replay,
        }
    }
    pub fn objects_mut(&mut self) -> ServerObjectsMut<'_> {
        ServerObjectsMut {
            objects: &mut self.state.objects,
        }
    }

    pub fn players(&self) -> ServerPlayers<'_> {
        ServerPlayers {
            state: &self.state.player_message_state,
        }
    }

    pub fn replay(&self) -> ServerReplay<'_> {
        ServerReplay {
            replay: &self.state.replay,
        }
    }

    pub fn objects(&self) -> ServerObjects<'_> {
        ServerObjects {
            objects: &self.state.objects,
        }
    }

    pub fn as_mut_parts(&mut self) -> ServerStateMutParts<'_> {
        ServerStateMutParts {
            players: ServerPlayersMut {
                state: &mut self.state.player_message_state,
            },
            objects: ServerObjectsMut {
                objects: &mut self.state.objects,
            },
            replay: ServerReplayMut {
                replay: &mut self.state.replay,
            },
        }
    }

    pub fn remove_player(&mut self, player_id: PlayerId) -> bool {
        self.state.remove_player(player_id, true)
    }

    pub fn spawn_skater(
        &mut self,
        player_id: PlayerId,
        team: Team,
        pos: Vec3,
        rot: Rot3,
        keep_stick_position: bool,
    ) -> bool {
        self.state
            .spawn_skater(player_id, team, pos, rot, keep_stick_position)
    }

    pub fn move_to_spectator(&mut self, player_id: PlayerId) -> bool {
        self.state.move_to_spectator(player_id)
    }

    /// Sends a server chat message to all players.
    pub fn add_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        self.players_mut().add_server_chat_message(message);
    }

    /// Sends a server chat message to a single player.
    pub fn add_directed_server_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver: PlayerId,
    ) {
        self.players_mut()
            .add_directed_server_chat_message(message, receiver);
    }

    /// Returns the player's name if the player is admin, otherwise sends a message telling the user to log in first.
    pub fn require_admin(&mut self, player_id: PlayerId) -> Option<Rc<str>> {
        self.players_mut()
            .check_admin_or_deny(player_id)
            .map(|player| player.name())
    }
}

#[derive(ReborrowCopyTraits)]
pub struct ServerState<'a> {
    state: &'a HQMServerState,
}

impl<'a> ServerState<'a> {
    /// Returns the position of a player's skater, if the player is currently on the ice.
    pub fn skater_position(&self, player_id: PlayerId) -> Option<Vec3> {
        Some(self.state.skater_for_player(player_id)?.body.pos)
    }

    /// Returns the position of the bottom of a player's skater, if on ice.
    pub fn skater_feet_position(&self, player_id: PlayerId) -> Option<Vec3> {
        let skater = self.state.skater_for_player(player_id)?;
        Some(skater.body.pos - skater.body.rot * Vec3::Y * skater.height)
    }

    /// Gets an immutable reference to player state.
    pub fn players(&self) -> ServerPlayers<'_> {
        ServerPlayers {
            state: &self.state.player_message_state,
        }
    }

    pub fn replay(&self) -> ServerReplay<'_> {
        ServerReplay {
            replay: &self.state.replay,
        }
    }

    pub fn objects(&self) -> ServerObjects<'_> {
        ServerObjects {
            objects: &self.state.objects,
        }
    }
}

/// A struct containing the individual parts of a [ServerStateMut].
///
/// This is useful if you want to mutably borrow several properties at once without getting in trouble with the borrow checker.
#[non_exhaustive]
pub struct ServerStateMutParts<'a> {
    pub players: ServerPlayersMut<'a>,
    pub objects: ServerObjectsMut<'a>,
    pub replay: ServerReplayMut<'a>,
}

#[derive(ReborrowTraits)]
#[Const(ServerObjects)]
pub struct ServerObjectsMut<'a> {
    objects: &'a mut [Option<GameObject>],
}

impl<'a> ServerObjectsMut<'a> {
    pub fn spawn_puck(&mut self, puck: Puck) -> Option<usize> {
        self.objects.spawn_puck(puck)
    }

    pub fn remove_all_pucks(&mut self) {
        for x in self.objects.iter_mut() {
            if matches!(x, Some(GameObject::Puck(_))) {
                *x = None;
            }
        }
    }

    pub fn get_puck(&self, object_index: usize) -> Option<&Puck> {
        self.objects.get(object_index).and_then(|x| match x {
            Some(GameObject::Puck(puck)) => Some(puck),
            _ => None,
        })
    }

    /// Iterates over every puck and its object index.
    pub fn pucks(&self) -> impl Iterator<Item = (usize, &Puck)> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| {
                let Some(GameObject::Puck(puck)) = object else {
                    return None;
                };
                Some((index, puck))
            })
    }

    /// Iterates mutably over every puck and its object index.
    pub fn pucks_mut(&mut self) -> impl Iterator<Item = (usize, &mut Puck)> {
        self.objects
            .iter_mut()
            .enumerate()
            .filter_map(|(index, object)| {
                let Some(GameObject::Puck(puck)) = object else {
                    return None;
                };
                Some((index, puck))
            })
    }

    pub fn get_puck_mut(&mut self, object_index: usize) -> Option<&mut Puck> {
        self.objects.get_mut(object_index).and_then(|x| match x {
            Some(GameObject::Puck(puck)) => Some(puck),
            _ => None,
        })
    }
}

#[derive(ReborrowCopyTraits)]
pub struct ServerObjects<'a> {
    objects: &'a [Option<GameObject>],
}

impl<'a> ServerObjects<'a> {
    pub fn get_puck(&self, object_index: usize) -> Option<&Puck> {
        self.objects.get(object_index).and_then(|x| match x {
            Some(GameObject::Puck(puck)) => Some(puck),
            _ => None,
        })
    }

    /// Iterates over every puck and its object index.
    pub fn pucks(&self) -> impl Iterator<Item = (usize, &Puck)> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| {
                let Some(GameObject::Puck(puck)) = object else {
                    return None;
                };
                Some((index, puck))
            })
    }
}

#[derive(ReborrowTraits)]
#[Const(ServerReplay)]
pub struct ServerReplayMut<'a> {
    #[reborrow]
    replay: &'a mut HQMTickHistory,
}

impl<'a> ServerReplayMut<'a> {
    /// Adds a replay to the replay queue.
    pub fn add_replay_to_queue(
        &mut self,
        start_step: u32,
        end_step: u32,
        force_view: Option<PlayerId>,
    ) {
        self.replay
            .add_replay_to_queue(start_step, end_step, force_view)
    }

    pub fn is_in_replay(&self) -> bool {
        self.replay.is_in_replay()
    }

    pub fn set_history_length(&mut self, history_length: usize) {
        self.replay.set_history_capacity(history_length);
    }

    pub fn game_step(&self) -> u32 {
        self.replay.game_step()
    }
}

#[derive(ReborrowCopyTraits)]
pub struct ServerReplay<'a> {
    replay: &'a HQMTickHistory,
}

impl<'a> ServerReplay<'a> {
    pub fn is_in_replay(&self) -> bool {
        self.replay.is_in_replay()
    }

    pub fn game_step(&self) -> u32 {
        self.replay.game_step()
    }
}

/// Mutable handle to player state.
#[derive(ReborrowTraits)]
#[Const(ServerPlayers)]
pub struct ServerPlayersMut<'a> {
    #[reborrow]
    state: &'a mut ServerPlayersAndMessages,
}

impl<'a> ServerPlayersMut<'a> {
    pub fn add_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        self.state.add_server_chat_message(message);
    }

    pub fn add_directed_server_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver_index: PlayerId,
    ) {
        self.state
            .add_directed_server_chat_message(message, receiver_index);
    }

    pub fn add_user_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        sender_index: PlayerId,
    ) {
        if self.state.players.get_player(sender_index).is_some() {
            self.state
                .add_user_chat_message(message, sender_index.index);
        }
    }

    pub fn add_goal_message(
        &mut self,
        team: Team,
        goal_player_index: Option<PlayerId>,
        assist_player_index: Option<PlayerId>,
    ) {
        self.state
            .add_goal_message(team, goal_player_index, assist_player_index);
    }

    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer<'_>> {
        self.state
            .players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns a iterator over the players in the server, that also allows changing the player objects.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = ServerPlayerMut<'_>> {
        self.state
            .players
            .iter_players_mut()
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer<'_>> {
        self.state
            .players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer<'_>> {
        self.state
            .players
            .get_player(id)
            .map(|player| ServerPlayer { id, player })
    }

    /// Returns a mutable handle to a player in the server.
    pub fn get_by_index_mut(&mut self, index: PlayerIndex) -> Option<ServerPlayerMut<'_>> {
        self.state
            .players
            .get_player_mut_by_index(index)
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_mut(&mut self, id: PlayerId) -> Option<ServerPlayerMut<'_>> {
        self.state
            .players
            .get_player_mut(id)
            .map(|player| ServerPlayerMut { id, player })
    }

    /// Returns a player object if the player is admin, otherwise sends a message telling the user to log in first.
    pub fn check_admin_or_deny(&mut self, player_id: PlayerId) -> Option<ServerPlayer<'_>> {
        self.state
            .players
            .check_admin_or_deny(player_id)
            .map(|player| ServerPlayer {
                id: player_id,
                player,
            })
    }

    /// Convenience method to count the number of players currently in the red or blue team.
    pub fn count_team_members(&self) -> (usize, usize) {
        let a = self.rb();
        a.count_team_members()
    }
}

/// Immutable handle to player state.
#[derive(ReborrowCopyTraits)]
pub struct ServerPlayers<'a> {
    state: &'a ServerPlayersAndMessages,
}

impl<'a> ServerPlayers<'a> {
    /// Returns a iterator over the players in the server.
    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer<'_>> {
        self.state
            .players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer<'_>> {
        self.state
            .players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer<'_>> {
        self.state
            .players
            .get_player(id)
            .map(|player| ServerPlayer { id, player })
    }

    /// Convenience method to count the number of players currently in the red or blue team.
    pub fn count_team_members(&self) -> (usize, usize) {
        let mut red_player_count = 0usize;
        let mut blue_player_count = 0usize;
        for player in self.iter() {
            if let Some(team) = player.team() {
                if team == Team::Red {
                    red_player_count += 1;
                } else if team == Team::Blue {
                    blue_player_count += 1;
                }
            }
        }
        (red_player_count, blue_player_count)
    }
}

/// Mutable handle to player who is connected to the server.
#[derive(ReborrowTraits)]
#[Const(ServerPlayer)]
pub struct ServerPlayerMut<'a> {
    pub id: PlayerId,
    #[reborrow]
    pub(crate) player: &'a mut InternalServerPlayer,
}

impl<'a> ServerPlayerMut<'a> {
    pub fn has_skater(&self) -> bool {
        self.player.has_skater()
    }

    pub fn team(&self) -> Option<Team> {
        self.player.team()
    }

    pub fn input(&self) -> &PlayerInput {
        &self.player.input
    }

    pub fn input_mut(&mut self) -> &mut PlayerInput {
        &mut self.player.input
    }

    pub fn is_admin(&self) -> bool {
        self.player.is_admin
    }

    pub fn name(&self) -> Rc<str> {
        self.player.player_name.clone()
    }

    pub fn add_directed_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        self.player.add_directed_server_chat_message(message);
    }

    pub fn player_type(&self) -> ServerPlayerType {
        match self.player.data {
            ServerPlayerData::NetworkPlayer { .. } => ServerPlayerType::Player,
            ServerPlayerData::Bot { .. } => ServerPlayerType::Bot,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ServerPlayerType {
    Player,
    Bot,
}

/// Immutable handle to player who is connected to the server.
#[derive(ReborrowCopyTraits)]
pub struct ServerPlayer<'a> {
    pub id: PlayerId,
    pub(crate) player: &'a InternalServerPlayer,
}

impl<'a> ServerPlayer<'a> {
    pub fn has_skater(&self) -> bool {
        self.player.has_skater()
    }

    pub fn team(&self) -> Option<Team> {
        self.player.team()
    }

    pub fn input(&self) -> &PlayerInput {
        &self.player.input
    }

    pub fn is_admin(&self) -> bool {
        self.player.is_admin
    }

    pub fn name(&self) -> Rc<str> {
        self.player.player_name.clone()
    }

    pub fn player_type(&self) -> ServerPlayerType {
        match self.player.data {
            ServerPlayerData::NetworkPlayer { .. } => ServerPlayerType::Player,
            ServerPlayerData::Bot { .. } => ServerPlayerType::Bot,
        }
    }
}

#[non_exhaustive]
pub enum ExitReason {
    Disconnected,
    Timeout,
    AdminKicked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puck_iterators_return_indices_and_allow_mutation() {
        let mut objects: [Option<GameObject>; 4] = std::array::from_fn(|_| None);
        objects[1] = Some(GameObject::Puck(Puck::new(Vec3::X, Rot3::IDENTITY)));
        objects[3] = Some(GameObject::Puck(Puck::new(Vec3::Z, Rot3::IDENTITY)));

        let indices: Vec<_> = ServerObjects { objects: &objects }
            .pucks()
            .map(|(index, _)| index)
            .collect();
        assert_eq!(indices, [1, 3]);

        let mut object_view = ServerObjectsMut {
            objects: &mut objects,
        };
        for (_, puck) in object_view.pucks_mut() {
            puck.radius = 0.25;
        }

        assert!(object_view.pucks().all(|(_, puck)| puck.radius == 0.25));
    }
}
