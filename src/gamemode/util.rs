use crate::game::{PlayerId, Rink, Team};
use crate::gamemode::{GameContext, ServerMessagesMut};
use glam::Vec3;
use glamx::Rot3;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, PI};
use std::rc::Rc;
use tracing::info;

/// Lets players join teams and spectate using the join and spectate keys.
///
/// Players who stop playing must wait [TeamSelection::team_switch_cooldown] ticks before they
/// can join a team again.
pub struct TeamSelection {
    /// Maximum number of players per team.
    pub team_max: usize,
    /// Number of ticks a player has to wait after spectating before they can join a team again.
    ///
    /// Set to 0 to let players rejoin immediately.
    pub team_switch_cooldown: u32,
    /// Players who get a chat message whenever someone joins a team or starts spectating.
    pub show_extra_messages: HashSet<PlayerId>,
    team_switch_timer: HashMap<PlayerId, u32>,
}

impl TeamSelection {
    pub const DEFAULT_TEAM_SWITCH_COOLDOWN: u32 = 500;

    pub fn new(team_max: usize) -> Self {
        Self {
            team_max,
            team_switch_cooldown: Self::DEFAULT_TEAM_SWITCH_COOLDOWN,
            show_extra_messages: HashSet::new(),
            team_switch_timer: HashMap::new(),
        }
    }

    /// Sets the number of ticks a player has to wait after spectating before they can join a team again.
    pub fn with_team_switch_cooldown(mut self, ticks: u32) -> Self {
        self.team_switch_cooldown = ticks;
        self
    }

    /// Moves players to and from teams based on their input, and returns the number of red and blue players.
    ///
    /// `coords` gives the spawn position and rotation for the n-th player joining a team this tick.
    pub fn update(
        &mut self,
        ctx: &mut GameContext,
        coords: impl Fn(&Rink, Team, usize) -> (Vec3, Rot3),
    ) -> (usize, usize) {
        let mut red_player_count = 0;
        let mut blue_player_count = 0;
        let mut spectating_players = SmallVec::<[_; 32]>::new();
        let mut joining_red = SmallVec::<[_; 32]>::new();
        let mut joining_blue = SmallVec::<[_; 32]>::new();
        self.team_switch_timer.retain(|_, x| {
            *x = x.saturating_sub(1);
            *x > 0
        });
        for player in ctx.players.iter() {
            let player_id = player.id;
            let input = player.input();
            if let Some(team) = player.team() {
                if input.spectate() {
                    spectating_players.push((player_id, player.name()))
                } else if team == Team::Red {
                    red_player_count += 1;
                } else {
                    blue_player_count += 1;
                }
            } else if !self.team_switch_timer.contains_key(&player_id) {
                if input.join_red() {
                    joining_red.push((player_id, player.name()));
                } else if input.join_blue() {
                    joining_blue.push((player_id, player.name()));
                }
            }
        }
        for (player_id, player_name) in spectating_players {
            info!("{} ({}) is spectating", player_name, player_id);
            self.force_spectate(ctx, player_id);
            self.send_extra_message(&mut ctx.messages, format!("{player_name} is spectating"));
        }

        let mut add_players =
            |players: SmallVec<[(PlayerId, Rc<str>); 32]>, team: Team, player_count: &mut usize| {
                for (i, (player_id, player_name)) in players.into_iter().enumerate() {
                    if *player_count >= self.team_max {
                        break;
                    }

                    let (pos, rot) = coords(ctx.rink, team, i);

                    if !ctx.spawn_skater(player_id, team, pos, rot, false) {
                        break;
                    }
                    info!("{} ({}) has joined team {:?}", player_name, player_id, team);
                    *player_count += 1;
                    self.send_extra_message(
                        &mut ctx.messages,
                        format!("{player_name} is playing for {team}"),
                    );
                }
            };

        add_players(joining_red, Team::Red, &mut red_player_count);
        add_players(joining_blue, Team::Blue, &mut blue_player_count);

        (red_player_count, blue_player_count)
    }

    /// Moves a player to the spectators and starts their team switch cooldown.
    ///
    /// Returns false if the player was not on the ice.
    pub fn force_spectate(&mut self, ctx: &mut GameContext, player_id: PlayerId) -> bool {
        if !ctx.move_to_spectator(player_id) {
            return false;
        }
        if self.team_switch_cooldown > 0 {
            self.team_switch_timer
                .insert(player_id, self.team_switch_cooldown);
        }
        true
    }

    /// Forgets everything about a player. Call this when a player leaves the server.
    pub fn remove_player(&mut self, player_id: PlayerId) {
        self.team_switch_timer.remove(&player_id);
        self.show_extra_messages.remove(&player_id);
    }

    fn send_extra_message(&self, messages: &mut ServerMessagesMut, message: String) {
        for player_id in self.show_extra_messages.iter() {
            messages.add_directed_server_chat_message(message.clone(), *player_id);
        }
    }
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum SpawnPoint {
    Center,
    Bench,
}

pub fn get_spawnpoint(rink: &Rink, team: Team, spawn_point: SpawnPoint) -> (Vec3, Rot3) {
    match team {
        Team::Red => match spawn_point {
            SpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) + 3.0, 0.0);
                let pos = Vec3::new(rink.width / 2.0, 2.0, z);
                let rot = Rot3::from_rotation_y(rot);
                (pos, rot)
            }
            SpawnPoint::Bench => {
                let z = (rink.length / 2.0) + 4.0;
                let pos = Vec3::new(0.5, 2.0, z);
                let rot = Rot3::from_rotation_y(3.0 * FRAC_PI_2);
                (pos, rot)
            }
        },
        Team::Blue => match spawn_point {
            SpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) - 3.0, PI);
                let pos = Vec3::new(rink.width / 2.0, 2.0, z);
                let rot = Rot3::from_rotation_y(rot);
                (pos, rot)
            }
            SpawnPoint::Bench => {
                let z = (rink.length / 2.0) - 4.0;
                let pos = Vec3::new(0.5, 2.0, z);
                let rot = Rot3::from_rotation_y(3.0 * FRAC_PI_2);
                (pos, rot)
            }
        },
    }
}
