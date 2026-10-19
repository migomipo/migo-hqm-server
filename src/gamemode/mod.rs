use crate::ServerConfiguration;
use crate::game::{
    PhysicsConfiguration, PhysicsEvent, PlayerId, PlayerIndex, PlayerInput, Puck, Rink,
    ScoreboardValues, SkaterObject, Team,
};
use crate::players::{
    PlayerListExt, PlayerSlots, ServerMessages, ServerPlayer as InternalServerPlayer,
    ServerPlayerData,
};
use crate::server::{
    GameObject, HQMTickHistory, ObjectExt, ServerInternals, ServerSplit, SkaterLinks,
};
use glam::Vec3;
use glamx::Rot3;
use std::borrow::Cow;
use std::cmp::PartialEq;
use std::rc::Rc;

pub mod shootout;
pub mod standard_match;
pub mod util;
pub mod warmup;

/// Specifies the server game behaviour.
///
/// To create a new game mode, you need to write an object that implements this trait. Every
/// hook that can change the server state gets a [GameContext].
pub trait GameMode {
    /// Called when the server starts.
    fn init(&mut self, _ctx: &mut GameContext) {}

    /// Called once each tick before the physics simulation is done.
    ///
    /// It is recommended that things like spawning pucks and moving players to and from teams are done before the physics simulation, so it should be done here.
    ///
    /// This is also called on ticks where a replay is shown instead of the physics simulation, so
    /// timers counted down here keep running during replays. Use [Replay::is_in_replay] to tell
    /// these ticks apart. No hooks are called at all while there are no players on the server.
    fn before_tick(&mut self, ctx: &mut GameContext);

    /// Called once each tick after the physics simulation is done.
    /// A list of physics events that occurred during the simulation is provided. Most of them have to do with the puck's movement.
    ///
    /// Changes to the server state are included in this tick's replication and recording data, but
    /// do not affect physics until the next tick.
    ///
    /// This is not called on ticks where a replay is shown. During those ticks, physics is paused,
    /// the game step does not advance, and the scoreboard values returned on the last simulated
    /// tick are shown.
    fn after_tick(&mut self, ctx: &mut GameContext, events: &[PhysicsEvent]) -> ScoreboardValues;

    /// Called when a chat message starting with "/" is received from a user. This method is called between ticks and not during, so you can do anything here.
    fn handle_command(
        &mut self,
        _ctx: &mut GameContext,
        _cmd: &str,
        _arg: &str,
        _player_index: PlayerId,
    ) {
    }

    /// Called when a new game begins, before the first [GameMode::before_tick] of that game.
    ///
    /// A new game begins on the first tick with a player on the server after the server has
    /// started, after [GameContext::new_game] was called, or after the previous game was
    /// abandoned. The server state, such as pucks and skaters, has been cleared at this point, so
    /// this is the place to reset game mode state and set up the new game.
    fn game_started(&mut self, _ctx: &mut GameContext) {}

    /// Called when the last player has left the server during a game, right before the game is
    /// reset.
    ///
    /// The server state of the abandoned game can still be read here. [GameMode::game_started] is
    /// called once a player joins again. Games that end through [GameContext::new_game] do not
    /// call this, as the game mode already knows about them.
    fn game_abandoned(&mut self, _ctx: &mut GameContext) {}

    /// Called right before a player is removed from the server.
    ///
    /// As the player has not yet been removed, it is possible to get the player object from the context.
    fn before_player_exit(
        &mut self,
        _ctx: &mut GameContext,
        _player_id: PlayerId,
        _reason: ExitReason,
    ) {
    }

    /// Called right after a new player has joined the server.
    fn after_player_join(&mut self, _ctx: &mut GameContext, _player_index: PlayerId) {}

    /// Gets the server team size that will be shown in the server list.
    fn server_list_team_size(&self) -> u32 {
        0
    }

    /// Whether the current tick should be written to the game recording, if recording is enabled.
    fn include_tick_in_recording(&self) -> bool {
        false
    }
}

/// Access to the server state, split into fields that can be borrowed independently.
///
/// This is the object you will mainly use to interact with the server. The fields can be used at
/// the same time, for example iterating over `players` while sending `messages` or changing
/// skaters in `objects`. Operations that change several parts at once, like spawning skaters or
/// starting a new game, are methods on the context itself and need the whole context.
///
/// Functions that only read the server state can take `&GameContext`.
#[non_exhaustive]
pub struct GameContext<'a> {
    pub players: Players<'a>,
    pub objects: Objects<'a>,
    pub messages: Messages<'a>,
    pub replay: Replay<'a>,
    pub rink: &'a mut Rink,
    pub config: &'a mut ServerConfiguration,
    /// Physics configuration. Changes take effect from the next physics simulation step.
    pub physics_config: &'a mut PhysicsConfiguration,
    internals: ServerInternals<'a>,
}

impl<'a> From<ServerSplit<'a>> for GameContext<'a> {
    fn from(split: ServerSplit<'a>) -> Self {
        Self {
            players: Players {
                players: split.players,
            },
            objects: Objects {
                objects: split.objects,
            },
            messages: Messages {
                messages: split.messages,
            },
            replay: Replay {
                replay: split.replay,
            },
            rink: split.rink,
            config: split.config,
            physics_config: split.physics_config,
            internals: split.internals,
        }
    }
}

impl GameContext<'_> {
    fn skater_links(&mut self) -> SkaterLinks<'_> {
        SkaterLinks {
            players: self.players.players,
            messages: self.messages.messages,
            objects: self.objects.objects,
        }
    }

    /// Ends the current game and resets the server state.
    ///
    /// Pucks, skaters and replay history are cleared immediately, and the recording of the game is
    /// saved if recording is enabled. [GameMode::game_started] is called on the next tick with a
    /// player on the server, so game mode state should be reset there.
    pub fn new_game(&mut self) {
        crate::server::new_game(
            &mut self.internals,
            self.config,
            self.players.players,
            self.messages.messages,
            self.objects.objects,
            self.replay.replay,
        );
    }

    /// Puts a player on the ice, or moves their skater if they are already on the ice.
    ///
    /// Returns false if the player does not exist or there is no free object slot.
    pub fn spawn_skater(
        &mut self,
        player_id: PlayerId,
        team: Team,
        pos: Vec3,
        rot: Rot3,
        keep_stick_position: bool,
    ) -> bool {
        self.skater_links()
            .spawn_skater(player_id, team, pos, rot, keep_stick_position)
    }

    /// Moves a player off the ice. Returns false if the player was not on the ice.
    pub fn move_to_spectator(&mut self, player_id: PlayerId) -> bool {
        self.skater_links().move_to_spectator(player_id)
    }

    /// Removes a player from the server. Returns false if the player does not exist.
    pub fn remove_player(&mut self, player_id: PlayerId) -> bool {
        self.skater_links().remove_player(player_id, true)
    }

    /// Returns the player's name if the player is admin, otherwise sends a message telling the user to log in first.
    pub fn require_admin(&mut self, player_id: PlayerId) -> Option<Rc<str>> {
        self.messages
            .messages
            .check_admin_or_deny(self.players.players, player_id)
            .map(|player| player.player_name.clone())
    }

    /// Announces a goal. Scorer and assist are left out if they are no longer on the server.
    pub fn add_goal_message(
        &mut self,
        team: Team,
        goal_player: Option<PlayerId>,
        assist_player: Option<PlayerId>,
    ) {
        self.messages.messages.add_goal_message(
            self.players.players,
            team,
            goal_player,
            assist_player,
        );
    }

    /// Sends a chat message to all players as if it was written by a player.
    ///
    /// Nothing is sent if the sender is not on the server.
    pub fn add_user_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        sender: PlayerId,
    ) {
        if self.players.players.get_player(sender).is_some() {
            self.messages
                .messages
                .add_user_chat_message(message, sender.index);
        }
    }
}

/// Pucks and skaters on the ice.
pub struct Objects<'a> {
    objects: &'a mut [Option<GameObject>],
}

impl Objects<'_> {
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

    pub fn get_puck_mut(&mut self, object_index: usize) -> Option<&mut Puck> {
        self.objects.get_mut(object_index).and_then(|x| match x {
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

    /// Returns a player's skater, if the player is currently on the ice.
    pub fn skater(&self, player_id: PlayerId) -> Option<&SkaterObject> {
        self.skaters()
            .find_map(|(id, skater)| (id == player_id).then_some(skater))
    }

    /// Returns a mutable reference to a player's skater, if the player is currently on the ice.
    ///
    /// See [SkaterObject] for what to keep in mind when changing its physics properties.
    pub fn skater_mut(&mut self, player_id: PlayerId) -> Option<&mut SkaterObject> {
        self.skaters_mut()
            .find_map(|(id, skater)| (id == player_id).then_some(skater))
    }

    /// Iterates over every skater and the player it belongs to.
    pub fn skaters(&self) -> impl Iterator<Item = (PlayerId, &SkaterObject)> {
        self.objects.iter().filter_map(|object| match object {
            Some(GameObject::Skater(player_id, skater)) => Some((*player_id, skater)),
            _ => None,
        })
    }

    /// Iterates mutably over every skater and the player it belongs to.
    ///
    /// See [SkaterObject] for what to keep in mind when changing its physics properties.
    pub fn skaters_mut(&mut self) -> impl Iterator<Item = (PlayerId, &mut SkaterObject)> {
        self.objects.iter_mut().filter_map(|object| match object {
            Some(GameObject::Skater(player_id, skater)) => Some((*player_id, skater)),
            _ => None,
        })
    }
}

/// Replay history and the replay queue.
pub struct Replay<'a> {
    replay: &'a mut HQMTickHistory,
}

impl Replay<'_> {
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

/// Sending messages to players.
///
/// This is separate from [Players], so messages can be sent while the player list is borrowed,
/// for example while iterating over the players.
pub struct Messages<'a> {
    messages: &'a mut ServerMessages,
}

impl Messages<'_> {
    /// Sends a server chat message to all players.
    pub fn add_server_chat_message(&mut self, message: impl Into<Cow<'static, str>>) {
        self.messages.add_server_chat_message(message);
    }

    /// Sends a server chat message to a single player.
    ///
    /// Nothing is sent if the player is not on the server.
    pub fn add_directed_server_chat_message(
        &mut self,
        message: impl Into<Cow<'static, str>>,
        receiver: PlayerId,
    ) {
        self.messages
            .add_directed_server_chat_message(message, receiver);
    }
}

/// The players connected to the server.
pub struct Players<'a> {
    players: &'a mut PlayerSlots,
}

impl Players<'_> {
    /// Returns an iterator over the players in the server.
    pub fn iter(&self) -> impl Iterator<Item = ServerPlayer<'_>> {
        self.players
            .iter_players()
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an iterator over the players in the server, that also allows changing the player objects.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = ServerPlayerMut<'_>> {
        self.players
            .iter_players_mut()
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get_by_index(&self, index: PlayerIndex) -> Option<ServerPlayer<'_>> {
        self.players
            .get_player_by_index(index)
            .map(|(id, player)| ServerPlayer { id, player })
    }

    /// Returns an immutable handle to a player in the server.
    pub fn get(&self, id: PlayerId) -> Option<ServerPlayer<'_>> {
        self.players
            .get_player(id)
            .map(|player| ServerPlayer { id, player })
    }

    /// Returns a mutable handle to a player in the server.
    pub fn get_by_index_mut(&mut self, index: PlayerIndex) -> Option<ServerPlayerMut<'_>> {
        self.players
            .get_player_mut_by_index(index)
            .map(|(id, player)| ServerPlayerMut { id, player })
    }

    /// Returns a mutable handle to a player in the server.
    pub fn get_mut(&mut self, id: PlayerId) -> Option<ServerPlayerMut<'_>> {
        self.players
            .get_player_mut(id)
            .map(|player| ServerPlayerMut { id, player })
    }

    /// Convenience method to count the number of players currently in the red or blue team.
    pub fn count_team_members(&self) -> (usize, usize) {
        let mut red_player_count = 0usize;
        let mut blue_player_count = 0usize;
        for player in self.iter() {
            match player.team() {
                Some(Team::Red) => red_player_count += 1,
                Some(Team::Blue) => blue_player_count += 1,
                None => {}
            }
        }
        (red_player_count, blue_player_count)
    }
}

/// Mutable handle to player who is connected to the server.
pub struct ServerPlayerMut<'a> {
    pub id: PlayerId,
    pub(crate) player: &'a mut InternalServerPlayer,
}

impl ServerPlayerMut<'_> {
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

    pub fn player_type(&self) -> ServerPlayerType {
        player_type(self.player)
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ServerPlayerType {
    Player,
    Bot,
}

fn player_type(player: &InternalServerPlayer) -> ServerPlayerType {
    match player.data {
        ServerPlayerData::NetworkPlayer { .. } => ServerPlayerType::Player,
        ServerPlayerData::Bot { .. } => ServerPlayerType::Bot,
    }
}

/// Immutable handle to player who is connected to the server.
#[derive(Copy, Clone)]
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

    pub fn input(&self) -> &'a PlayerInput {
        &self.player.input
    }

    pub fn is_admin(&self) -> bool {
        self.player.is_admin
    }

    pub fn name(&self) -> Rc<str> {
        self.player.player_name.clone()
    }

    pub fn player_type(&self) -> ServerPlayerType {
        player_type(self.player)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExitReason {
    Disconnected,
    Timeout,
    AdminKicked,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::HQMServerState;

    #[test]
    fn puck_iterators_return_indices_and_allow_mutation() {
        let mut objects: [Option<GameObject>; 4] = std::array::from_fn(|_| None);
        objects[1] = Some(GameObject::Puck(Puck::new(Vec3::X, Rot3::IDENTITY)));
        objects[3] = Some(GameObject::Puck(Puck::new(Vec3::Z, Rot3::IDENTITY)));

        let mut object_view = Objects {
            objects: &mut objects,
        };
        let indices: Vec<_> = object_view.pucks().map(|(index, _)| index).collect();
        assert_eq!(indices, [1, 3]);

        for (_, puck) in object_view.pucks_mut() {
            puck.radius = 0.25;
        }

        assert!(object_view.pucks().all(|(_, puck)| puck.radius == 0.25));
    }

    #[test]
    fn skaters_are_found_by_player_and_can_be_changed_while_reading_players() {
        let mut state = HQMServerState::new();
        let on_ice = state
            .player_message_state
            .add_player("on ice", "127.0.0.1:1".parse().unwrap())
            .unwrap();
        let spectator = state
            .player_message_state
            .add_player("spectator", "127.0.0.1:2".parse().unwrap())
            .unwrap();
        state
            .objects
            .spawn_puck(Puck::new(Vec3::ZERO, Rot3::IDENTITY));
        assert!(state.skater_links().spawn_skater(
            on_ice,
            Team::Blue,
            Vec3::X,
            Rot3::IDENTITY,
            false
        ));

        let players = Players {
            players: &mut state.player_message_state.players,
        };
        let mut objects = Objects {
            objects: &mut state.objects,
        };
        assert!(objects.skater(spectator).is_none());
        for (id, skater) in objects.skaters_mut() {
            if players.get(id).and_then(|p| p.team()) == Some(Team::Blue) {
                skater.body.linear_velocity = Vec3::Z;
            }
        }

        let skater = objects.skater(on_ice).unwrap();
        assert_eq!(skater.body.pos, Vec3::X);
        assert_eq!(skater.body.linear_velocity, Vec3::Z);
        assert_eq!(objects.skaters().count(), 1);
    }

    #[test]
    fn messages_can_be_sent_while_iterating_players() {
        let mut state = HQMServerState::new();
        let player_id = state
            .player_message_state
            .add_player("test", "127.0.0.1:1".parse().unwrap())
            .unwrap();

        let players = Players {
            players: &mut state.player_message_state.players,
        };
        let mut messages = Messages {
            messages: &mut state.player_message_state.messages,
        };
        for player in players.iter() {
            messages
                .add_directed_server_chat_message(format!("Hello {}", player.name()), player.id);
        }

        state.player_message_state.flush_messages();
        let player = state
            .player_message_state
            .players
            .get_player(player_id)
            .unwrap();
        let ServerPlayerData::NetworkPlayer { data } = &player.data else {
            panic!("expected a network player");
        };
        assert!(data.messages.iter().any(|message| matches!(
            message.as_ref(),
            crate::players::HQMMessage::Chat { message, .. } if message == "Hello test"
        )));
    }
}
