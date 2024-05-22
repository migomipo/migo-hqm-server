use crate::game::{
    PhysicsEvent, PlayerId, PlayerIndex, PlayerInput, Puck, Rink, ScoreboardValues, SkaterObject,
    Team,
};
use crate::server::{
    HQMServer, HQMServerPlayer, HQMServerState, PlayerListExt, ServerStatePlayerItem,
};
use crate::ServerConfiguration;
use nalgebra::{Point3, Rotation3};
use reborrow::{ReborrowCopyTraits, ReborrowTraits};
use std::borrow::Cow;
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

    fn save_replay_data(&self, _server: ServerMut) -> bool {
        false
    }
}

/// A struct containing the individual parts of a [ServerMut].
///
/// This is useful if you want to mutably borrow several properties at once without getting in trouble with the borrow checker.
#[non_exhaustive]
pub struct ServerMutParts<'a> {
    pub state: ServerStateMut<'a>,
    pub scoreboard: &'a mut ScoreboardValues,
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
    pub fn as_mut_parts(&mut self) -> ServerMutParts {
        ServerMutParts {
            state: ServerStateMut {
                state: &mut self.server.state,
            },
            scoreboard: &mut self.server.scoreboard,
            rink: &mut self.server.rink,
            config: &mut self.server.config,
        }
    }
    /// Gets an immutable reference to player and puck state.
    pub fn state(&self) -> ServerState {
        ServerState {
            state: &self.server.state,
        }
    }

    /// Gets a mutable reference to player and puck state.
    pub fn state_mut(&mut self) -> ServerStateMut {
        ServerStateMut {
            state: &mut self.server.state,
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
        &self.server.scoreboard
    }

    pub fn scoreboard_mut(&mut self) -> &mut ScoreboardValues {
        &mut self.server.scoreboard
    }

    pub fn game_step(&self) -> u32 {
        self.server.game_step
    }

    /// Adds a replay to the replay queue.
    pub fn add_replay_to_queue(
        &mut self,
        start_step: u32,
        end_step: u32,
        force_view: Option<PlayerId>,
    ) {
        self.server
            .add_replay_to_queue(start_step, end_step, force_view)
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

impl<'a> Server<'a> {
    /// Gets an immutable reference to player and puck state.
    pub fn state(&self) -> ServerState {
        ServerState {
            state: &self.server.state,
        }
    }
    pub fn rink(&self) -> &Rink {
        &self.server.rink
    }
    pub fn scoreboard(&self) -> &ScoreboardValues {
        &self.server.scoreboard
    }

    pub fn game_step(&self) -> u32 {
        self.server.game_step
    }

    pub fn config(&self) -> &ServerConfiguration {
        &self.server.config
    }
}

/// A struct containing the individual parts of a [ServerStateMut].
///
/// This is useful if you want to mutably borrow several properties at once without getting in trouble with the borrow checker.
#[non_exhaustive]
pub struct ServerStateMutParts<'a> {
    pub players: ServerPlayerListMut<'a>,
    pub pucks: &'a mut [Option<Puck>],
}

/// Mutable handle to puck and player state.
#[derive(ReborrowTraits)]
#[Const(ServerState)]
pub struct ServerStateMut<'a> {
    #[reborrow]
    state: &'a mut HQMServerState,
}

impl<'a> ServerStateMut<'a> {
    pub fn as_mut_parts(&mut self) -> ServerStateMutParts {
        ServerStateMutParts {
            players: ServerPlayerListMut {
                players: &mut self.state.players,
            },
            pucks: &mut self.state.pucks,
        }
    }

    pub fn players(&self) -> ServerPlayerList {
        ServerPlayerList {
            players: &self.state.players,
        }
    }

    pub fn players_mut(&mut self) -> ServerPlayerListMut {
        ServerPlayerListMut {
            players: &mut self.state.players,
        }
    }

    pub fn pucks(&self) -> &[Option<Puck>] {
        &self.state.pucks
    }

    pub fn pucks_mut(&mut self) -> &mut [Option<Puck>] {
        &mut self.state.pucks
    }

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

    pub fn spawn_puck(&mut self, puck: Puck) -> Option<usize> {
        if let Some(object_index) = self.state.pucks.iter().position(|x| x.is_none()) {
            self.state.pucks[object_index] = Some(puck);
            Some(object_index)
        } else {
            None
        }
    }

    pub fn remove_all_pucks(&mut self) {
        for x in self.state.pucks.iter_mut() {
            *x = None;
        }
    }

    pub fn get_puck(&self, index: usize) -> Option<&Puck> {
        self.state.get_puck(index)
    }

    pub fn get_puck_mut(&mut self, index: usize) -> Option<&mut Puck> {
        self.state.get_puck_mut(index)
    }
}

/// Immutable handle to puck and player state.
#[derive(ReborrowCopyTraits)]
pub struct ServerState<'a> {
    state: &'a HQMServerState,
}

impl<'a> ServerState<'a> {
    pub fn players(&self) -> ServerPlayerList {
        ServerPlayerList {
            players: &self.state.players,
        }
    }
    pub fn pucks(&self) -> &[Option<Puck>] {
        &self.state.pucks
    }

    pub fn get_puck(&self, index: usize) -> Option<&Puck> {
        self.state.get_puck(index)
    }
}

/// Mutable list of players in the server.
#[derive(ReborrowTraits)]
#[Const(ServerPlayerList)]
pub struct ServerPlayerListMut<'a> {
    players: &'a mut [ServerStatePlayerItem],
}

impl<'a> ServerPlayerListMut<'a> {
    /// Returns an iterator over the players in the server.
    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer> {
        self.players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns a iterator over the players in the server, that also allows changing the player objects.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = ServerPlayerMut> {
        self.players
            .iter_players_mut()
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer> {
        self.players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer> {
        self.players
            .get_player(id)
            .map(|player| ServerPlayer { id, player })
    }

    /// Returns a mutable handle to a player in the server.
    pub fn get_by_index_mut(&mut self, index: PlayerIndex) -> Option<ServerPlayerMut> {
        self.players
            .get_player_mut_by_index(index)
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_mut(&mut self, id: PlayerId) -> Option<ServerPlayerMut> {
        self.players
            .get_player_mut(id)
            .map(|player| ServerPlayerMut { id, player })
    }

    /// Returns a player object if the player is admin, otherwise sends a message telling the user to log in first.
    pub fn check_admin_or_deny(&mut self, player_id: PlayerId) -> Option<ServerPlayer> {
        self.players
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

/// Immutable list of players in the server.
#[derive(ReborrowCopyTraits)]
pub struct ServerPlayerList<'a> {
    players: &'a [ServerStatePlayerItem],
}

impl<'a> ServerPlayerList<'a> {
    /// Returns a iterator over the players in the server.
    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer> {
        self.players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer> {
        self.players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer> {
        self.players
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

    pub fn input(&self) -> PlayerInput {
        self.player.input.clone()
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

    pub fn input(&self) -> PlayerInput {
        self.player.input.clone()
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
