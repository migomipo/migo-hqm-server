use crate::game::{PlayerId, Rink, Team};
use crate::gamemode::ServerPlayersMut;
use nalgebra::{Point3, Rotation3};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, PI};
use std::rc::Rc;
use tracing::info;

pub fn add_players<
    F1: Fn(Team, usize) -> (Point3<f32>, Rotation3<f32>),
    FSpectate: FnMut(PlayerId),
    FJoin: FnMut(PlayerId, Team),
>(
    mut server: ServerPlayersMut,
    team_max: usize,
    team_switch_timer: &mut HashMap<PlayerId, u32>,
    show_extra_messages: Option<&HashSet<PlayerId>>,
    coords: F1,
    mut on_spectate: FSpectate,
    mut on_join: FJoin,
) -> (usize, usize) {
    let mut red_player_count = 0;
    let mut blue_player_count = 0;
    let mut spectating_players = SmallVec::<[_; 32]>::new();
    let mut joining_red = SmallVec::<[_; 32]>::new();
    let mut joining_blue = SmallVec::<[_; 32]>::new();
    for player in server.iter() {
        let player_id = player.id;
        let input = player.input();
        let team = player.team();
        if let Some(x) = team_switch_timer
            .get_mut(&player_id) {
            *x = x.saturating_sub(1)
        }
        if let Some(team) = team {
            if input.spectate() {
                team_switch_timer.insert(player_id, 500);
                spectating_players.push((player_id, player.name()))
            } else if team == Team::Red {
                red_player_count += 1;
            } else {
                blue_player_count += 1;
            }
        } else if (input.join_red() || input.join_blue())
            && team_switch_timer.get(&player_id).is_none_or(|x| *x == 0)
        {
            if input.join_red() {
                joining_red.push((player_id, player.name()));
            } else if input.join_blue() {
                joining_blue.push((player_id, player.name()));
            }
        }
    }
    for (player_id, player_name) in spectating_players {
        info!("{} ({}) is spectating", player_name, player_id);
        server.move_to_spectator(player_id);
        on_spectate(player_id);
        if let Some(show_extra_messages) = show_extra_messages {
            let s = format!("{player_name} is spectating");
            for i in show_extra_messages.iter() {
                server.add_directed_server_chat_message(s.clone(), *i);
            }
        }
    }

    let mut add_players =
        |players: SmallVec<[(PlayerId, Rc<str>); 32]>, team: Team, player_count: &mut usize| {
            for (i, (player_id, player_name)) in players.into_iter().enumerate() {
                if *player_count >= team_max {
                    break;
                }

                let (pos, rot) = coords(team, i);

                let res = server.spawn_skater(player_id, team, pos, rot, false);

                if res {
                    info!("{} ({}) has joined team {:?}", player_name, player_id, team);
                    *player_count += 1;
                    on_join(player_id, team);
                    if let Some(show_extra_messages) = show_extra_messages {
                        let s = format!("{player_name} is playing for Red");
                        for msg_player_id in show_extra_messages.iter() {
                            server.add_directed_server_chat_message(s.clone(), *msg_player_id);
                        }
                    }
                } else {
                    break;
                }
            }
        };

    add_players(joining_red, Team::Red, &mut red_player_count);
    add_players(joining_blue, Team::Blue, &mut blue_player_count);

    (red_player_count, blue_player_count)
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum SpawnPoint {
    Center,
    Bench,
}

pub fn get_spawnpoint(
    rink: &Rink,
    team: Team,
    spawn_point: SpawnPoint,
) -> (Point3<f32>, Rotation3<f32>) {
    match team {
        Team::Red => match spawn_point {
            SpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) + 3.0, 0.0);
                let pos = Point3::new(rink.width / 2.0, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, rot, 0.0);
                (pos, rot)
            }
            SpawnPoint::Bench => {
                let z = (rink.length / 2.0) + 4.0;
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            }
        },
        Team::Blue => match spawn_point {
            SpawnPoint::Center => {
                let (z, rot) = ((rink.length / 2.0) - 3.0, PI);
                let pos = Point3::new(rink.width / 2.0, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, rot, 0.0);
                (pos, rot)
            }
            SpawnPoint::Bench => {
                let z = (rink.length / 2.0) - 4.0;
                let pos = Point3::new(0.5, 2.0, z);
                let rot = Rotation3::from_euler_angles(0.0, 3.0 * FRAC_PI_2, 0.0);
                (pos, rot)
            }
        },
    }
}
