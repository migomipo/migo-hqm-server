//! The player list shown to clients during replays.
//!
//! Clients show the name and team colour of a skater from the entry in the player list that owns
//! its object slot. The players on the server can have changed since a replayed tick was
//! recorded, so during a replay the list shows the skater owners of the replayed tick instead.
//! Players who have left are shown by placeholders, which are removed again when the replay ends.

use std::rc::Rc;

use crate::game::{PlayerId, PlayerIndex, Team};
use crate::server::players::{MAX_PLAYERS, PlayerUpdateData};
use crate::server::replay::SkaterOwners;
use crate::server::{ObjectSlot, PlayerListExt, PlayerSlots, ServerMessages};

pub(crate) struct ReplayPlayers {
    /// Whether the player list of a replay is shown.
    active: bool,
    placeholders: Vec<Placeholder>,
}

/// An entry in the player list for a player who has left, shown while their skater is replayed.
struct Placeholder {
    /// The player who left.
    player_id: PlayerId,
    /// The index the placeholder is shown at, which is reserved while it is shown.
    index: PlayerIndex,
    player_name: Rc<str>,
}

impl ReplayPlayers {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            placeholders: vec![],
        }
    }

    /// Shows the player list for a replayed tick with the given skater owners, or for the live
    /// game if `owners` is `None`.
    pub(crate) fn update(
        &mut self,
        players: &mut PlayerSlots,
        messages: &mut ServerMessages,
        owners: Option<&SkaterOwners>,
    ) {
        match owners {
            Some(owners) => {
                self.active = true;
                self.add_placeholders(players, owners);
            }
            None if self.active => self.clear(players),
            None => return,
        }

        let mut shown: [Option<PlayerUpdateData>; MAX_PLAYERS] = std::array::from_fn(|_| None);
        for (player_id, player) in players.iter_players() {
            let object = match owners {
                Some(owners) => owned_skater(owners, player_id),
                None => player.skater_assignment(),
            };
            shown[player_id.index.0] = Some(PlayerUpdateData {
                player_name: player.player_name.clone(),
                object,
            });
        }
        if let Some(owners) = owners {
            for placeholder in &self.placeholders {
                shown[placeholder.index.0] = Some(PlayerUpdateData {
                    player_name: placeholder.player_name.clone(),
                    object: owned_skater(owners, placeholder.player_id),
                });
            }
        }
        messages.show_players(&shown);
    }

    /// Adds placeholders for the skater owners who are no longer on the server.
    fn add_placeholders(&mut self, players: &mut PlayerSlots, owners: &SkaterOwners) {
        for owner in owners.iter().flatten() {
            let player_id = owner.player_id;
            if players.get_player(player_id).is_some()
                || self.placeholder_index(player_id).is_some()
            {
                continue;
            }
            // The index the player had is taken if it is free, so the placeholder takes their
            // place in the list. If the server is full, the skater is shown without an owner.
            let index = if players.is_free(player_id.index) {
                Some(player_id.index)
            } else {
                players.find_empty_slot()
            };
            let Some(index) = index else {
                continue;
            };
            players.set_reserved(index, true);
            self.placeholders.push(Placeholder {
                player_id,
                index,
                player_name: owner.player_name.clone(),
            });
        }
    }

    fn placeholder_index(&self, player_id: PlayerId) -> Option<PlayerIndex> {
        self.placeholders
            .iter()
            .find(|placeholder| placeholder.player_id == player_id)
            .map(|placeholder| placeholder.index)
    }

    /// The index of the entry that shows the skater of a player in the replay, if any.
    pub(crate) fn shown_index(
        &self,
        players: &PlayerSlots,
        player_id: PlayerId,
    ) -> Option<PlayerIndex> {
        if players.get_player(player_id).is_some() {
            Some(player_id.index)
        } else {
            self.placeholder_index(player_id)
        }
    }

    /// Removes the placeholders and gives their indices back. The players are not told, which
    /// is left to the next [ReplayPlayers::update] or to the new game.
    pub(crate) fn clear(&mut self, players: &mut PlayerSlots) {
        for placeholder in self.placeholders.drain(..) {
            players.set_reserved(placeholder.index, false);
        }
        self.active = false;
    }
}

/// The skater a player owned in a replayed tick, and the team it was on.
fn owned_skater(owners: &SkaterOwners, player_id: PlayerId) -> Option<(ObjectSlot, Team)> {
    owners.iter().enumerate().find_map(|(slot, owner)| {
        owner
            .as_ref()
            .filter(|owner| owner.player_id == player_id)
            .map(|owner| (ObjectSlot(slot), owner.team))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use glam::Vec3;
    use glamx::Rot3;

    use crate::game::{PlayerId, PlayerIndex, Team};
    use crate::server::test_server::{EmptyGameMode, Received, TestServer};

    /// The player list a client shows after the messages it was sent, by player index.
    type PlayerList = BTreeMap<usize, (String, Option<Team>)>;

    fn apply(list: &mut PlayerList, messages: Vec<Received>) {
        for message in messages {
            match message {
                Received::PlayerUpdate { index, name, team } => {
                    list.insert(index.0, (name, team));
                }
                Received::PlayerRemoved { index } => {
                    list.remove(&index.0);
                }
                _ => {}
            }
        }
    }

    fn entry(name: &str, team: Option<Team>) -> (String, Option<Team>) {
        (name.to_owned(), team)
    }

    fn server() -> TestServer<EmptyGameMode> {
        let mut t = TestServer::new(EmptyGameMode);
        t.ctx().replay.set_history_length(100);
        t
    }

    fn spawn(t: &mut TestServer<EmptyGameMode>, player_id: PlayerId, team: Team) {
        assert!(
            t.ctx()
                .spawn_skater(player_id, team, Vec3::ZERO, Rot3::IDENTITY, false)
        );
    }

    /// Plays a tick of the live game, and returns its game step.
    fn record(t: &mut TestServer<EmptyGameMode>) -> u32 {
        t.tick();
        t.ctx().replay.game_step()
    }

    /// Plays one tick of a replay of `step`, and returns the forced view it was sent with.
    fn replay(
        t: &mut TestServer<EmptyGameMode>,
        step: u32,
        force_view: Option<PlayerId>,
    ) -> Option<PlayerIndex> {
        assert_eq!(
            t.ctx().replay.add_replay_to_queue(step, step, force_view),
            1
        );
        t.server.advance_tick(&mut t.mode).unwrap().forced_view
    }

    fn is_free(t: &TestServer<EmptyGameMode>, index: PlayerIndex) -> bool {
        t.server.state.player_message_state.players.is_free(index)
    }

    #[test]
    fn a_player_who_left_is_shown_by_a_placeholder_during_the_replay() {
        let mut t = server();
        let watcher = t.join("watcher");
        let scorer = t.join("scorer");
        spawn(&mut t, scorer, Team::Red);
        let step = record(&mut t);
        t.leave(scorer);
        t.take_messages(watcher);
        let recorded = t
            .server
            .state
            .player_message_state
            .recording_message_count();

        // The placeholder takes the place the player had.
        assert_eq!(replay(&mut t, step, Some(scorer)), Some(scorer.index));
        assert_eq!(
            t.take_messages(watcher),
            [Received::PlayerUpdate {
                index: scorer.index,
                name: "scorer".to_owned(),
                team: Some(Team::Red),
            }]
        );
        assert_eq!(
            t.server
                .state
                .player_message_state
                .recording_message_count(),
            recorded
        );

        // A player who joins during the replay does not get the placeholder's index.
        let late = t.join("late");
        assert_ne!(late.index, scorer.index);
        t.take_messages(watcher);

        t.tick();
        assert_eq!(
            t.take_messages(watcher),
            [Received::PlayerRemoved {
                index: scorer.index
            }]
        );
        assert!(is_free(&t, scorer.index));
    }

    #[test]
    fn players_still_on_the_server_are_shown_with_their_replayed_skaters() {
        let mut t = server();
        let watcher = t.join("watcher");
        let a = t.join("a");
        let b = t.join("b");
        spawn(&mut t, a, Team::Red);
        let step = record(&mut t);
        // b gets the object slot that a had in the replayed tick.
        t.ctx().move_to_spectator(a);
        spawn(&mut t, b, Team::Blue);
        record(&mut t);
        let mut list = PlayerList::new();
        apply(&mut list, t.take_messages(watcher));
        let live = list.clone();
        assert_eq!(live[&a.index.0], entry("a", None));
        assert_eq!(live[&b.index.0], entry("b", Some(Team::Blue)));

        assert_eq!(replay(&mut t, step, Some(a)), Some(a.index));
        // b lets go of the skater before a claims it.
        assert_eq!(
            t.take_messages(watcher),
            [
                Received::PlayerUpdate {
                    index: b.index,
                    name: "b".to_owned(),
                    team: None,
                },
                Received::PlayerUpdate {
                    index: a.index,
                    name: "a".to_owned(),
                    team: Some(Team::Red),
                },
            ]
        );

        t.tick();
        apply(&mut list, t.take_messages(watcher));
        assert_eq!(list, live);
    }

    #[test]
    fn a_placeholder_whose_index_was_taken_is_shown_at_a_free_index() {
        let mut t = server();
        let watcher = t.join("watcher");
        let scorer = t.join("scorer");
        spawn(&mut t, scorer, Team::Blue);
        let step = record(&mut t);
        t.leave(scorer);
        let newcomer = t.join("newcomer");
        assert_eq!(newcomer.index, scorer.index);
        let mut list = PlayerList::new();
        apply(&mut list, t.take_messages(watcher));

        let placeholder = replay(&mut t, step, Some(scorer)).expect("the scorer is viewed");
        assert_ne!(placeholder, newcomer.index);
        apply(&mut list, t.take_messages(watcher));
        assert_eq!(list[&placeholder.0], entry("scorer", Some(Team::Blue)));
        assert_eq!(list[&newcomer.index.0], entry("newcomer", None));

        t.tick();
        apply(&mut list, t.take_messages(watcher));
        assert!(!list.contains_key(&placeholder.0));
    }

    #[test]
    fn a_player_who_joins_during_a_replay_is_shown_the_replay_players() {
        let mut t = server();
        let watcher = t.join("watcher");
        let scorer = t.join("scorer");
        spawn(&mut t, scorer, Team::Red);
        let step = record(&mut t);
        t.leave(scorer);
        replay(&mut t, step, None);

        let late = t.join("late");
        let mut list = PlayerList::new();
        apply(&mut list, t.take_messages(late));
        assert_eq!(
            list,
            PlayerList::from([
                (watcher.index.0, entry("watcher", None)),
                (scorer.index.0, entry("scorer", Some(Team::Red))),
                (late.index.0, entry("late", None)),
            ])
        );

        t.tick();
        apply(&mut list, t.take_messages(late));
        assert!(!list.contains_key(&scorer.index.0));
    }

    #[test]
    fn a_new_game_during_a_replay_removes_the_placeholders() {
        let mut t = server();
        let watcher = t.join("watcher");
        let scorer = t.join("scorer");
        spawn(&mut t, scorer, Team::Red);
        let step = record(&mut t);
        t.leave(scorer);
        t.ctx().replay.add_replay_to_queue(step, step, None);
        t.ctx().replay.add_replay_to_queue(step, step, None);
        t.tick();
        let mut list = PlayerList::new();
        apply(&mut list, t.take_messages(watcher));
        assert!(list.contains_key(&scorer.index.0));

        t.server.new_game();
        t.tick();
        apply(&mut list, t.take_messages(watcher));
        assert_eq!(
            list,
            PlayerList::from([(watcher.index.0, entry("watcher", None))])
        );
        assert!(is_free(&t, scorer.index));
    }

    #[test]
    fn team_chat_during_a_replay_keeps_the_replayed_skater() {
        let mut t = server();
        let a = t.join("a");
        let mate = t.join("mate");
        spawn(&mut t, a, Team::Red);
        let step = record(&mut t);
        spawn(&mut t, a, Team::Blue);
        spawn(&mut t, mate, Team::Blue);
        record(&mut t);
        replay(&mut t, step, None);
        let mut list = PlayerList::new();
        apply(&mut list, t.take_messages(mate));
        assert_eq!(list[&a.index.0], entry("a", Some(Team::Red)));

        t.chat(a, "/t hello");
        let messages = t.take_messages(mate);
        assert!(messages.contains(&Received::PlayerUpdate {
            index: a.index,
            name: "[Blue] a".to_owned(),
            team: Some(Team::Red),
        }));
        apply(&mut list, messages);
        assert_eq!(list[&a.index.0], entry("a", Some(Team::Red)));
    }
}
