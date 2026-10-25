use crate::game::{HALF_TURN_Y, PlayerId, PlayerIndex, Puck, Rink, Team};
use crate::gamemode::GameContext;
use glam::Vec3;
use glamx::Rot3;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
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
    team_switch_timer: HashMap<PlayerId, u32>,
}

impl TeamSelection {
    pub const DEFAULT_TEAM_SWITCH_COOLDOWN: u32 = 500;

    pub fn new(team_max: usize) -> Self {
        Self {
            team_max,
            team_switch_cooldown: Self::DEFAULT_TEAM_SWITCH_COOLDOWN,
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

    /// Handles the admin command `/fs <player index>`, which moves a player to the spectators,
    /// starts their team switch cooldown and announces it.
    pub fn handle_force_spectate_command(
        &mut self,
        ctx: &mut GameContext,
        admin_id: PlayerId,
        arg: &str,
    ) {
        let Some(admin_name) = ctx.require_admin(admin_id) else {
            return;
        };
        let Some(target) = arg
            .parse::<PlayerIndex>()
            .ok()
            .and_then(|index| ctx.players.get_by_index(index))
        else {
            ctx.messages
                .add_directed_server_chat_message("No player with this ID exists", admin_id);
            return;
        };
        let (target_id, target_name) = (target.id, target.name());
        if self.force_spectate(ctx, target_id) {
            info!(
                "{} ({}) forced {} ({}) off ice",
                admin_name, admin_id, target_name, target_id
            );
            ctx.messages
                .add_server_chat_message(format!("{target_name} forced off ice by {admin_name}"));
        } else {
            ctx.messages.add_directed_server_chat_message(
                format!("{target_name} is not on the ice"),
                admin_id,
            );
        }
    }

    /// Forgets everything about a player. Call this when a player leaves the server.
    ///
    /// A player who left without this being called is forgotten anyway once their team switch
    /// cooldown runs out.
    pub fn remove_player(&mut self, player_id: PlayerId) {
        self.team_switch_timer.remove(&player_id);
    }
}

/// Whether a game goes on after a tick, or has finished and should be replaced by a new one.
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub(crate) enum GameProgress {
    Continue,
    Finished,
}

/// Puts `count` warmup pucks in a row across the center line, `altitude` meters above the ice.
pub fn spawn_warmup_pucks(ctx: &mut GameContext, count: usize, altitude: f32) {
    let (width, length) = (ctx.rink.width, ctx.rink.length);
    let first_x = width / 2.0 - 0.4 * count.saturating_sub(1) as f32;
    for index in 0..count {
        let pos = Vec3::new(first_x + 0.8 * index as f32, altitude, length / 2.0);
        ctx.objects.spawn_puck(Puck::new(pos, Rot3::IDENTITY));
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
                let z = (rink.length / 2.0) + 3.0;
                let pos = Vec3::new(rink.width / 2.0, 2.0, z);
                (pos, Rot3::IDENTITY)
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
                let z = (rink.length / 2.0) - 3.0;
                let pos = Vec3::new(rink.width / 2.0, 2.0, z);
                (pos, HALF_TURN_Y)
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
