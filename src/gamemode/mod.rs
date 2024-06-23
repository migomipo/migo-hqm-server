use crate::game::{
    PhysicsEvent, PlayerId, PlayerIndex, PlayerInput, Puck, Rink, ScoreboardValues, SkaterObject,
    Team,
};
use crate::server::{
    HQMServer, HQMServerPlayer, HQMServerPlayersAndMessages, HQMTickHistory, PlayerListExt,
    ServerPlayerData,
};
use crate::ServerConfiguration;
use nalgebra::{Point3, Rotation3};
use reborrow::{ReborrowCopyTraits, ReborrowTraits};
use std::borrow::Cow;
use std::cmp::PartialEq;
use std::rc::Rc;

pub mod russian;
pub mod shootout;
pub mod util;
pub mod warmup;

mod match_commands;
mod match_util;
pub mod standard_match;

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
    /// You can update the score, add chat messages and so on, but you should not move players to and from teams, or spawn new objects.
    fn after_tick(&mut self, server: ServerMut, events: &[PhysicsEvent]);

    /// Called when a chat message starting with "/" is received from a user. This method is called between ticks and not during, so you can do anything here.
    fn handle_command(
        &mut self,
        _server: ServerMut,
        _cmd: &str,
        _arg: &str,
        _player_index: PlayerId,
    ) {
    }

    fn get_initial_game_values(&mut self) -> InitialGameValues;
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
    fn server_list_team_size(&self) -> u32;

    fn include_tick_in_recording(&self, _server: Server) -> bool {
        false
    }
}

/// A struct containing the individual parts of a [ServerMut].
///
/// This is useful if you want to mutably borrow several properties at once without getting in trouble with the borrow checker.
#[non_exhaustive]
pub struct ServerMutParts<'a> {
    pub players: ServerPlayersMut<'a>,
    pub scoreboard: &'a mut ScoreboardValues,
    pub rink: &'a mut Rink,
    pub config: &'a mut ServerConfiguration,
    pub pucks: &'a mut [Option<Puck>],
    pub replay: &'a mut HQMTickHistory,
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
    pub fn as_mut_parts(&mut self) -> ServerMutParts {
        ServerMutParts {
            players: ServerPlayersMut {
                state: &mut self.server.state.players,
            },
            scoreboard: &mut self.server.state.scoreboard,
            rink: &mut self.server.rink,
            config: &mut self.server.config,
            pucks: self.server.state.pucks.as_mut_slice(),
            replay: &mut self.server.state.replay,
        }
    }
    /// Gets an immutable reference to player state.
    pub fn players(&self) -> ServerPlayers {
        ServerPlayers {
            state: &self.server.state.players,
        }
    }

    /// Gets a mutable reference to player state.
    pub fn players_mut(&mut self) -> ServerPlayersMut {
        ServerPlayersMut {
            state: &mut self.server.state.players,
        }
    }

    pub fn pucks(&self) -> &[Option<Puck>] {
        self.server.state.pucks.as_slice()
    }

    pub fn pucks_mut(&mut self) -> &mut [Option<Puck>] {
        self.server.state.pucks.as_mut_slice()
    }

    pub fn replay(&self) -> ServerReplay {
        ServerReplay {
            replay: &self.server.state.replay,
        }
    }

    pub fn replay_mut(&mut self) -> ServerReplayMut {
        ServerReplayMut {
            replay: &mut self.server.state.replay,
        }
    }

    pub fn new_game(&mut self, v: InitialGameValues) {
        self.server.new_game(v)
    }

    pub fn rink(&self) -> &Rink {
        &self.server.rink
    }

    pub fn rink_mut(&mut self) -> &mut Rink {
        &mut self.server.rink
    }

    pub fn scoreboard(&self) -> &ScoreboardValues {
        &self.server.state.scoreboard
    }

    pub fn scoreboard_mut(&mut self) -> &mut ScoreboardValues {
        &mut self.server.state.scoreboard
    }

    pub fn set_history_length(&mut self, v: usize) {
        self.server.history_length = v;
    }

    pub fn config(&self) -> &ServerConfiguration {
        &self.server.config
    }

    pub fn config_mut(&mut self) -> &mut ServerConfiguration {
        &mut self.server.config
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
    pub fn players(&self) -> ServerPlayers {
        ServerPlayers {
            state: &self.server.state.players,
        }
    }
    pub fn rink(&self) -> &Rink {
        &self.server.rink
    }
    pub fn scoreboard(&self) -> &ScoreboardValues {
        &self.server.state.scoreboard
    }

    pub fn config(&self) -> &ServerConfiguration {
        &self.server.config
    }

    pub fn pucks(&self) -> &[Option<Puck>] {
        self.server.state.pucks.as_slice()
    }

    pub fn replay(&self) -> ServerReplay {
        ServerReplay {
            replay: &self.server.state.replay,
        }
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

    pub fn game_step(&self) -> u32 {
        self.replay.game_step
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
        self.replay.game_step
    }
}

/// Mutable handle to player state.
#[derive(ReborrowTraits)]
#[Const(ServerPlayers)]
pub struct ServerPlayersMut<'a> {
    #[reborrow]
    state: &'a mut HQMServerPlayersAndMessages,
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

    pub fn spawn_skater(
        &mut self,
        player_index: PlayerId,
        team: Team,
        pos: Point3<f32>,
        rot: Rotation3<f32>,
        keep_stick_position: bool,
    ) -> bool {
        self.state
            .spawn_skater(player_index, team, pos, rot, keep_stick_position)
    }

    pub fn move_to_spectator(&mut self, player_id: PlayerId) -> bool {
        self.state.move_to_spectator(player_id)
    }

    pub fn add_bot(&mut self, player_name: &str) -> Option<PlayerId> {
        self.state.add_bot(player_name)
    }

    pub fn remove_player(&mut self, player_id: PlayerId) -> bool {
        self.state.remove_player(player_id, true)
    }

    pub fn remove_bots(&mut self) {
        let p: Vec<_> = self
            .iter()
            .filter_map(|x| {
                if x.player_type() == ServerPlayerType::Bot {
                    Some(x.id)
                } else {
                    None
                }
            })
            .collect();
        for player_id in p {
            self.state.remove_player(player_id, true);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer> {
        self.state
            .players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns a iterator over the players in the server, that also allows changing the player objects.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = ServerPlayerMut> {
        self.state
            .players
            .iter_players_mut()
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer> {
        self.state
            .players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer> {
        self.state
            .players
            .get_player(id)
            .map(|player| ServerPlayer { id, player })
    }

    /// Returns a mutable handle to a player in the server.
    pub fn get_by_index_mut(&mut self, index: PlayerIndex) -> Option<ServerPlayerMut> {
        self.state
            .players
            .get_player_mut_by_index(index)
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_mut(&mut self, id: PlayerId) -> Option<ServerPlayerMut> {
        self.state
            .players
            .get_player_mut(id)
            .map(|player| ServerPlayerMut { id, player })
    }

    /// Returns a player object if the player is admin, otherwise sends a message telling the user to log in first.
    pub fn check_admin_or_deny(&mut self, player_id: PlayerId) -> Option<ServerPlayer> {
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

/// Immutable handle to player state.
#[derive(ReborrowCopyTraits)]
pub struct ServerPlayers<'a> {
    state: &'a HQMServerPlayersAndMessages,
}

impl<'a> ServerPlayers<'a> {
    /// Returns a iterator over the players in the server.
    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer> {
        self.state
            .players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer> {
        self.state
            .players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer> {
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
    pub(crate) player: &'a mut HQMServerPlayer,
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

    pub fn skater(&self) -> Option<(Team, &SkaterObject)> {
        self.player
            .object
            .as_ref()
            .map(|(_, skater, team)| (*team, skater))
    }

    pub fn skater_mut(&mut self) -> Option<(Team, &mut SkaterObject)> {
        self.player
            .object
            .as_mut()
            .map(|(_, skater, team)| (*team, skater))
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
    pub(crate) player: &'a HQMServerPlayer,
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

    pub fn skater(&self) -> Option<(Team, &SkaterObject)> {
        self.player
            .object
            .as_ref()
            .map(|(_, skater, team)| (*team, skater))
    }

    pub fn player_type(&self) -> ServerPlayerType {
        match self.player.data {
            ServerPlayerData::NetworkPlayer { .. } => ServerPlayerType::Player,
            ServerPlayerData::Bot { .. } => ServerPlayerType::Bot,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InitialGameValues {
    pub values: ScoreboardValues,
    pub puck_slots: usize,
}

#[non_exhaustive]
pub enum ExitReason {
    Disconnected,
    Timeout,
    AdminKicked,
}

pub trait PuckExt {
    fn spawn_puck(&mut self, puck: Puck) -> Option<usize>;

    fn remove_all_pucks(&mut self);

    fn get_puck(&self, index: usize) -> Option<&Puck>;

    fn get_puck_mut(&mut self, index: usize) -> Option<&mut Puck>;
}

impl PuckExt for [Option<Puck>] {
    fn spawn_puck(&mut self, puck: Puck) -> Option<usize> {
        if let Some(object_index) = self.iter().position(|x| x.is_none()) {
            self[object_index] = Some(puck);
            Some(object_index)
        } else {
            None
        }
    }

    fn remove_all_pucks(&mut self) {
        for x in self.iter_mut() {
            *x = None;
        }
    }

    fn get_puck(&self, index: usize) -> Option<&Puck> {
        self.get(index).map(|x| x.as_ref()).flatten()
    }

    fn get_puck_mut(&mut self, index: usize) -> Option<&mut Puck> {
        self.get_mut(index).map(|x| x.as_mut()).flatten()
    }
}
