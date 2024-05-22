use nalgebra::{Point3, Rotation3, Vector3};
use std::collections::HashMap;
use tracing::info;

use crate::game::{PhysicsEvent, PlayerId};
use crate::game::{PlayerIndex, Puck, ScoreboardValues, Team};
use crate::gamemode::util::add_players;
use crate::gamemode::{ExitReason, GameMode, InitialGameValues, ServerMut, ServerMutParts};
use crate::physics;
use reborrow::ReborrowMut;
use std::f32::consts::FRAC_PI_2;

#[derive(Debug, Clone)]
enum RussianStatus {
    WaitingForGame,
    Game {
        in_zone: Team,
        round: u32,
        goal_scored: bool,
    },
    GameOver {
        timer: u32,
    },
}

pub struct RussianGameMode {
    attempts: u32,
    status: RussianStatus,
    team_switch_timer: HashMap<PlayerId, u32>,
    team_max: usize,
}

impl RussianGameMode {
    pub fn new(attempts: u32, team_max: usize) -> Self {
        RussianGameMode {
            attempts,
            status: RussianStatus::WaitingForGame,
            team_switch_timer: Default::default(),
            team_max,
        }
    }

    fn update_players(&mut self, mut server: ServerMut) {
        let ServerMutParts { state, rink, .. } = server.as_mut_parts();
        let rink = &*rink;
        add_players(
            state,
            self.team_max,
            &mut self.team_switch_timer,
            None,
            move |team, _| {
                let mid_z = rink.length / 2.0;
                let z = match team {
                    Team::Red => mid_z + 12.0,
                    Team::Blue => mid_z - 12.0,
                };
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            },
            |_| {},
            |_, _| {},
        );
    }

    fn place_puck_for_team(&mut self, mut server: ServerMut, team: Team) {
        server.state_mut().remove_all_pucks();

        let z = match team {
            Team::Red => 55.0,
            Team::Blue => 6.0,
        };
        let puck_pos = Point3::new(server.rink().width / 2.0, 0.5, z);

        server
            .state_mut()
            .spawn_puck(Puck::new(puck_pos, Rotation3::identity()));

        self.fix_status(server, team);
    }

    fn fix_status(&mut self, mut server: ServerMut, team: Team) {
        match &mut self.status {
            RussianStatus::WaitingForGame => {
                self.status = RussianStatus::Game {
                    in_zone: team,
                    round: 0,
                    goal_scored: false,
                };

                let remaining_attempts = self.attempts;
                let msg = if remaining_attempts >= 2 {
                    format!("{} attempts left for {}", remaining_attempts, team)
                } else if remaining_attempts == 1 {
                    format!("Last attempt for {}", team)
                } else {
                    format!("Tie-breaker round for {}", team)
                };
                server.state_mut().add_server_chat_message(msg);
            }
            RussianStatus::Game { in_zone, round, .. } => {
                if *in_zone != team {
                    server.scoreboard_mut().time = 2000;
                    *in_zone = team;
                    if team == Team::Red {
                        *round += 1;
                    }
                    let remaining_attempts = self.attempts.saturating_sub(*round);
                    let msg = if remaining_attempts >= 2 {
                        format!("{} attempts left for {}", remaining_attempts, team)
                    } else if remaining_attempts == 1 {
                        format!("Last attempt for {}", team)
                    } else {
                        format!("Tie-breaker round for {}", team)
                    };
                    server.state_mut().add_server_chat_message(msg);
                }
            }
            RussianStatus::GameOver { .. } => {}
        }
    }

    fn init(&mut self, mut server: ServerMut) {
        server.scoreboard_mut().period = 1;
        server.scoreboard_mut().time = 2000;

        server.state_mut().remove_all_pucks();

        let s = format!("Each team will get {} attempts", self.attempts);
        server.state_mut().add_server_chat_message(s);

        let mut red_players = vec![];
        let mut blue_players = vec![];

        self.place_puck_for_team(server.rb_mut(), Team::Red);

        for player in server.state().players().iter() {
            let player_id = player.id;
            if let Some(team) = player.team() {
                if team == Team::Red {
                    red_players.push(player_id);
                } else if team == Team::Blue {
                    blue_players.push(player_id);
                }
            }
        }

        let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
        let length = server.rink().length;
        for (index, player_id) in red_players.into_iter().enumerate() {
            let z = (length / 2.0) + (12.0 + index as f32);
            let pos = Point3::new(0.5, 2.0, z);
            server
                .state_mut()
                .spawn_skater(player_id, Team::Red, pos, rot.clone(), false);
        }
        for (index, player_id) in blue_players.into_iter().enumerate() {
            let z = (length / 2.0) - (12.0 + index as f32);
            let pos = Point3::new(0.5, 2.0, z);
            server
                .state_mut()
                .spawn_skater(player_id, Team::Blue, pos, rot.clone(), false);
        }
    }

    fn check_ending(&mut self, game: &mut ScoreboardValues) {
        if let RussianStatus::Game { in_zone, round, .. } = self.status {
            let red_attempts_taken = round + if in_zone == Team::Blue { 1 } else { 0 };
            let blue_attempts_taken = round;
            let attempts = self.attempts.max(red_attempts_taken);
            let remaining_red_attempts = attempts - red_attempts_taken;
            let remaining_blue_attempts = attempts - blue_attempts_taken;

            let game_over = if let Some(difference) = game.red_score.checked_sub(game.blue_score) {
                remaining_blue_attempts < difference
            } else if let Some(difference) = game.blue_score.checked_sub(game.red_score) {
                remaining_red_attempts < difference
            } else {
                false
            };
            if game_over {
                self.status = RussianStatus::GameOver { timer: 500 };
                game.game_over = true;
            }
        }
    }

    fn reset_game(&mut self, mut server: ServerMut, player_id: PlayerId) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(player_id)
        {
            let name = player.name();
            info!("{} ({}) reset game", name, player_id);
            let msg = format!("Game reset by {}", name);

            server.new_game(self.get_initial_game_values());

            server.state_mut().add_server_chat_message(msg);
        }
    }

    fn force_player_off_ice(
        &mut self,
        mut server: ServerMut,
        admin_player_id: PlayerId,
        force_player_index: PlayerIndex,
    ) {
        if let Some(player) = server
            .state_mut()
            .players_mut()
            .check_admin_or_deny(admin_player_id)
        {
            let admin_player_name = player.name();

            if let Some(force_player) = server.state().players().get_by_index(force_player_index) {
                let force_player_id = force_player.id;
                let force_player_name = force_player.name();
                if server.state_mut().move_to_spectator(force_player_id) {
                    let msg = format!(
                        "{} forced off ice by {}",
                        force_player_name, admin_player_name
                    );
                    info!(
                        "{} ({}) forced {} ({}) off ice",
                        admin_player_name, admin_player_id, force_player_name, force_player_index
                    );
                    server.state_mut().add_server_chat_message(msg);
                    self.team_switch_timer.insert(force_player_id, 500);
                }
            }
        }
    }
}

impl GameMode for RussianGameMode {
    fn before_tick(&mut self, server: ServerMut) {
        self.update_players(server);
    }

    fn after_tick(&mut self, mut server: ServerMut, events: &[PhysicsEvent]) {
        let (red_player_count, blue_player_count) = {
            let mut red_player_count = 0usize;
            let mut blue_player_count = 0usize;

            let ServerMutParts {
                mut state, rink, ..
            } = server.as_mut_parts();

            for mut player in state.players_mut().iter_mut() {
                if let Some((team, skater)) = player.skater_mut() {
                    if team == Team::Red {
                        red_player_count += 1;
                    } else if team == Team::Blue {
                        blue_player_count += 1;
                    }
                    let (line, normal) = if team == Team::Red {
                        red_player_count += 1;
                        (&rink.red_zone_blue_line, Vector3::z_axis())
                    } else {
                        blue_player_count += 1;
                        (&rink.blue_zone_blue_line, -Vector3::z_axis())
                    };

                    let p = Point3::new(0.0, 0.0, line.z);
                    for collision_ball in skater.collision_balls.iter_mut() {
                        let pos = &collision_ball.pos;
                        let radius = collision_ball.radius;
                        let overlap = (&p - pos).dot(&normal) + radius;
                        if overlap > 0.0 {
                            let mut new = normal.scale(overlap * 0.03125)
                                - collision_ball.velocity.scale(0.25);
                            if new.dot(&normal) > 0.0 {
                                physics::limit_friction(&mut new, &normal, 0.01);

                                collision_ball.velocity += new;
                            }
                        }
                    }
                }
            }

            (red_player_count, blue_player_count)
        };

        if let RussianStatus::WaitingForGame = self.status {
            let values = server.scoreboard_mut();
            if red_player_count > 0 && blue_player_count > 0 {
                values.time = values.time.saturating_sub(1);
                if values.time == 0 {
                    self.init(server.rb_mut());
                }
            } else {
                values.time = 1000;
            }
        } else if let RussianStatus::GameOver { timer } = &mut self.status {
            *timer = timer.saturating_sub(1);
            if *timer == 0 {
                server.new_game(self.get_initial_game_values());
            }
        } else if let RussianStatus::Game {
            in_zone,
            round,
            goal_scored,
        } = self.status
        {
            if goal_scored {
                let values = server.scoreboard_mut();
                values.goal_message_timer = values.goal_message_timer.saturating_sub(1);
                if values.goal_message_timer == 0 {
                    values.time = 2000;
                    self.place_puck_for_team(server.rb_mut(), in_zone);

                    self.status = RussianStatus::Game {
                        in_zone,
                        round,
                        goal_scored: false,
                    };
                }
            } else {
                for event in events {
                    match event {
                        PhysicsEvent::PuckEnteredNet { team: net_team, .. } => {
                            let team = net_team.get_other_team();
                            let values = server.scoreboard_mut();
                            // Goal!
                            match team {
                                Team::Red => {
                                    values.red_score += 1;
                                }
                                Team::Blue => {
                                    values.blue_score += 1;
                                }
                            };
                            self.status = RussianStatus::Game {
                                in_zone,
                                round,
                                goal_scored: true,
                            };
                            values.goal_message_timer = 300;
                            server.state_mut().add_goal_message(team, None, None);
                            self.check_ending(server.scoreboard_mut());
                        }
                        PhysicsEvent::PuckTouch { player, .. } => {
                            if let Some(player) = server.state().players().get(*player) {
                                if let Some(touching_team) = player.team() {
                                    self.fix_status(server.rb_mut(), touching_team);
                                }
                            }
                        }
                        PhysicsEvent::PuckEnteredOffensiveZone { team, .. } => {
                            let other_team = team.get_other_team();
                            self.fix_status(server.rb_mut(), other_team);
                        }
                        PhysicsEvent::PuckPassedDefensiveLine { .. } => {
                            self.check_ending(server.scoreboard_mut());
                        }
                        PhysicsEvent::PuckPassedGoalLine { .. } => {
                            self.check_ending(server.scoreboard_mut());
                        }
                        _ => {}
                    }
                }
                let values = server.scoreboard_mut();
                values.time = values.time.saturating_sub(1);
                if values.time == 0 {
                    self.check_ending(values);
                    match self.status {
                        RussianStatus::Game { in_zone, .. } => {
                            let other_team = in_zone.get_other_team();
                            self.place_puck_for_team(server, other_team);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn handle_command(&mut self, server: ServerMut, cmd: &str, arg: &str, player_index: PlayerId) {
        match cmd {
            "reset" | "resetgame" => {
                self.reset_game(server, player_index);
            }
            "fs" => {
                if let Ok(force_player_index) = arg.parse::<PlayerIndex>() {
                    self.force_player_off_ice(server, player_index, force_player_index);
                }
            }
            _ => {}
        }
    }

    fn get_initial_game_values(&mut self) -> InitialGameValues {
        InitialGameValues {
            values: ScoreboardValues {
                time: 1000,
                ..Default::default()
            },
            puck_slots: 1,
        }
    }

    fn game_started(&mut self, _server: ServerMut) {
        self.status = RussianStatus::WaitingForGame;
    }

    fn before_player_exit(&mut self, _server: ServerMut, player_id: PlayerId, _reason: ExitReason) {
        self.team_switch_timer.remove(&player_id);
    }

    fn server_list_team_size(&self) -> u32 {
        self.team_max as u32
    }

    fn save_replay_data(&self, _server: ServerMut) -> bool {
        !matches!(self.status, RussianStatus::WaitingForGame)
    }
}
