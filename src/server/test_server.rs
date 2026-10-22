//! A server without a network for testing game modes.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;

use super::{HQMServer, test_server};
use crate::game::{PhysicsEvent, PlayerId, PlayerIndex, PlayerInput, ScoreboardValues, Team};
use crate::gamemode::{GameContext, GameMode};
use crate::players::{HQMMessage, PlayerListExt, PlayerSlots, ServerPlayerData};

/// The only client version the server accepts.
const CLIENT_VERSION: u32 = 55;
/// Key bits checked by [PlayerInput::join_red] and [PlayerInput::join_blue].
const JOIN_RED_KEY: u32 = 0x4;
const JOIN_BLUE_KEY: u32 = 0x8;

/// Runs a game mode on a server without a network, for tests.
///
/// Players join, chat, leave and tick through the same code as on the real server, so hooks,
/// built-in commands and messages behave as they would in a real game. Players stay connected
/// as if their clients kept sending input, until they [leave](TestServer::leave).
///
/// The server and the game mode are public, so tests can also set up or inspect their state
/// directly.
pub(crate) struct TestServer<M: GameMode> {
    pub(crate) server: HQMServer,
    pub(crate) mode: M,
    players: HashMap<PlayerId, TestPlayer>,
    next_port: u16,
}

struct TestPlayer {
    addr: SocketAddr,
    /// The game when the messages were last read. A new game clears the message queues.
    game_id: u32,
    read: usize,
    unread: Vec<Received>,
}

/// A message a player has received, in a form that is easy to compare in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Received {
    PlayerUpdate {
        index: PlayerIndex,
        name: String,
        team: Option<Team>,
    },
    PlayerRemoved {
        index: PlayerIndex,
    },
    Goal {
        team: Team,
        scorer: Option<PlayerIndex>,
        assist: Option<PlayerIndex>,
    },
    Chat {
        from: Option<PlayerIndex>,
        text: String,
    },
}

impl From<&HQMMessage> for Received {
    fn from(message: &HQMMessage) -> Self {
        match message {
            HQMMessage::PlayerUpdate {
                player_index,
                data: Some(data),
            } => Received::PlayerUpdate {
                index: *player_index,
                name: data.player_name.to_string(),
                team: data.object.map(|(_, team)| team),
            },
            HQMMessage::PlayerUpdate {
                player_index,
                data: None,
            } => Received::PlayerRemoved {
                index: *player_index,
            },
            HQMMessage::Goal {
                team,
                goal_player_index,
                assist_player_index,
            } => Received::Goal {
                team: *team,
                scorer: *goal_player_index,
                assist: *assist_player_index,
            },
            HQMMessage::Chat {
                player_index,
                message,
            } => Received::Chat {
                from: *player_index,
                text: message.to_string(),
            },
        }
    }
}

/// The message queue of a network player, or `None` for bots and players not on the server.
fn message_queue(players: &PlayerSlots, player_id: PlayerId) -> Option<&[Rc<HQMMessage>]> {
    match &players.get_player(player_id)?.data {
        ServerPlayerData::NetworkPlayer { data } => Some(&data.messages),
        ServerPlayerData::Bot { .. } => None,
    }
}

/// All messages in a player's message queue, for tests that do not run a [TestServer].
///
/// Panics if the player is not a network player on the server.
pub(crate) fn received(players: &PlayerSlots, player_id: PlayerId) -> Vec<Received> {
    message_queue(players, player_id)
        .expect("expected a network player on the server")
        .iter()
        .map(|message| Received::from(&**message))
        .collect()
}

/// The text of the chat messages among `messages`.
pub(crate) fn chats(messages: impl IntoIterator<Item = Received>) -> Vec<String> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            Received::Chat { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

impl<M: GameMode> TestServer<M> {
    /// Starts a server with default settings and calls [GameMode::init].
    pub(crate) fn new(mut mode: M) -> Self {
        let mut server = test_server();
        mode.init(&mut server.context());
        server.start_requested_game(&mut mode);
        Self {
            server,
            mode,
            players: HashMap::new(),
            next_port: 1,
        }
    }

    /// Connects a player, as if a client had joined. Panics if the player could not join.
    pub(crate) fn join(&mut self, name: &str) -> PlayerId {
        self.try_join(name)
            .unwrap_or_else(|| panic!("{name} could not join"))
    }

    /// Connects a player, as if a client had joined, and returns `None` if the server did not
    /// let the player in.
    pub(crate) fn try_join(&mut self, name: &str) -> Option<PlayerId> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.next_port));
        self.next_port += 1;
        self.server
            .player_join(addr, CLIENT_VERSION, name.to_owned(), &mut self.mode);
        let (player_id, _) = self
            .server
            .state
            .player_message_state
            .players
            .find_player_by_addr(addr)?;
        self.players.insert(
            player_id,
            TestPlayer {
                addr,
                game_id: self.server.game_id,
                read: 0,
                unread: vec![],
            },
        );
        self.collect_messages();
        Some(player_id)
    }

    /// Connects a player who is logged in as admin.
    pub(crate) fn join_admin(&mut self, name: &str) -> PlayerId {
        let player_id = self.join(name);
        self.server
            .state
            .player_message_state
            .players
            .get_player_mut(player_id)
            .unwrap()
            .is_admin = true;
        player_id
    }

    /// Disconnects a player who joined with [TestServer::join], as if their client had exited.
    pub(crate) fn leave(&mut self, player_id: PlayerId) {
        let player = self
            .players
            .remove(&player_id)
            .expect("only players who joined through the test server can leave");
        self.server.player_exit(player.addr, &mut self.mode);
        self.collect_messages();
    }

    /// The input of a player or bot. It stays as set until it is changed.
    pub(crate) fn input(&mut self, player_id: PlayerId) -> &mut PlayerInput {
        &mut self
            .server
            .state
            .player_message_state
            .players
            .get_player_mut(player_id)
            .expect("player is not on the server")
            .input
    }

    /// Holds down the key for joining a team.
    pub(crate) fn press_join(&mut self, player_id: PlayerId, team: Team) {
        self.input(player_id).keys |= match team {
            Team::Red => JOIN_RED_KEY,
            Team::Blue => JOIN_BLUE_KEY,
        };
    }

    /// Sends a chat message or command from a player, as if it was typed in the client.
    pub(crate) fn chat(&mut self, player_id: PlayerId, message: &str) {
        self.server
            .process_message(message.to_owned(), player_id, &mut self.mode);
        self.collect_messages();
    }

    /// Runs one server tick, and returns its scoreboard. Returns `None` if no players are on the
    /// server, as the server then skips the tick.
    pub(crate) fn tick(&mut self) -> Option<ScoreboardValues> {
        for player_id in self.players.keys() {
            let players = &mut self.server.state.player_message_state.players;
            if let Some(player) = players.get_player_mut(*player_id)
                && let ServerPlayerData::NetworkPlayer { data } = &mut player.data
            {
                data.inactivity = 0;
            }
        }
        let output = self.server.advance_tick(&mut self.mode);
        self.collect_messages();
        output.map(|output| output.scoreboard)
    }

    pub(crate) fn ticks(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.tick();
        }
    }

    /// Ticks until `done` returns true, and returns how many ticks that took. Panics if it takes
    /// more than `max_ticks`.
    pub(crate) fn tick_until(
        &mut self,
        max_ticks: u32,
        mut done: impl FnMut(&Self) -> bool,
    ) -> u32 {
        for ticks in 0..max_ticks {
            if done(self) {
                return ticks;
            }
            self.tick();
        }
        assert!(done(self), "condition not reached within {max_ticks} ticks");
        max_ticks
    }

    pub(crate) fn ctx(&mut self) -> GameContext<'_> {
        self.server.context()
    }

    /// Takes the messages a player has received since the last call.
    ///
    /// Messages that a new game cleared from the player's queue between two steps of the test,
    /// such as in the same tick, are not seen, just as a real client would never receive them.
    pub(crate) fn take_messages(&mut self, player_id: PlayerId) -> Vec<Received> {
        self.collect_messages();
        let player = self
            .players
            .get_mut(&player_id)
            .expect("only players who joined through the test server receive messages");
        std::mem::take(&mut player.unread)
    }

    /// Takes the text of the chat messages a player has received since the last call to this or
    /// [TestServer::take_messages].
    pub(crate) fn take_chats(&mut self, player_id: PlayerId) -> Vec<String> {
        chats(self.take_messages(player_id))
    }

    fn collect_messages(&mut self) {
        let state = &mut self.server.state.player_message_state;
        state.flush_messages();
        for (player_id, test_player) in &mut self.players {
            let Some(queue) = message_queue(&state.players, *player_id) else {
                continue;
            };
            if test_player.game_id != self.server.game_id {
                test_player.game_id = self.server.game_id;
                test_player.read = 0;
            }
            test_player.unread.extend(
                queue[test_player.read..]
                    .iter()
                    .map(|message| Received::from(&**message)),
            );
            test_player.read = queue.len();
        }
    }
}

/// A game mode that does nothing, for tests of the server itself.
pub(crate) struct EmptyGameMode;

impl GameMode for EmptyGameMode {
    fn before_tick(&mut self, _ctx: &mut GameContext) {}

    fn after_tick(&mut self, _ctx: &mut GameContext, _events: &[PhysicsEvent]) -> ScoreboardValues {
        ScoreboardValues::default()
    }
}
