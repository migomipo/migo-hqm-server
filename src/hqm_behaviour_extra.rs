use crate::hqm_game::{HQMGameWorld, HQMObjectIndex, HQMPuck, HQMTeam};
use crate::hqm_server::{
    HQMServer, HQMServerPlayerData, HQMServerPlayerIndex, HQMServerPlayerList,
};
use nalgebra::{Point3, Vector3};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMDualControlSetting {
    No,
    Yes,
    Combined,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMIcingConfiguration {
    Off,
    Touch,
    NoTouch,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMOffsideConfiguration {
    Off,
    Delayed,
    Immediate,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum HQMOffsideLineConfiguration {
    OffensiveBlue,
    Center,
}

#[derive(PartialEq, Debug, Clone)]
pub enum HQMIcingStatus {
    No,                               // No icing
    NotTouched(HQMTeam, Point3<f32>), // Puck has entered offensive half, but not reached the goal line
    Warning(HQMTeam, Point3<f32>),    // Puck has reached the goal line, delayed icing
    Icing(HQMTeam),                   // Icing has been called
}

#[derive(PartialEq, Debug, Clone)]
pub enum HQMOffsideStatus {
    Neutral,                                             // No offside
    InOffensiveZone(HQMTeam),                            // No offside, puck in offensive zone
    Warning(HQMTeam, Point3<f32>, HQMServerPlayerIndex), // Warning, puck entered offensive zone in an offside situation but not touched yet
    Offside(HQMTeam),                                    // Offside has been called
}

#[derive(Debug, Clone)]
pub struct HQMPuckTouch {
    pub player_index: HQMServerPlayerIndex,
    pub skater_index: HQMObjectIndex,
    pub team: HQMTeam,
    pub puck_pos: Point3<f32>,
    pub puck_speed: f32,
    pub first_time: u32,
    pub last_time: u32,
}

pub fn add_touch(
    puck: &HQMPuck,
    entry: Entry<HQMObjectIndex, VecDeque<HQMPuckTouch>>,
    player_index: HQMServerPlayerIndex,
    skater_index: HQMObjectIndex,
    team: HQMTeam,
    time: u32,
) {
    let puck_pos = puck.body.pos.clone();
    let puck_speed = puck.body.linear_velocity.norm();

    let touches = entry.or_insert_with(|| VecDeque::new());
    let most_recent_touch = touches.front_mut();

    match most_recent_touch {
        Some(most_recent_touch)
            if most_recent_touch.player_index == player_index && most_recent_touch.team == team =>
        {
            most_recent_touch.puck_pos = puck_pos;
            most_recent_touch.last_time = time;
            most_recent_touch.puck_speed = puck_speed;
        }
        _ => {
            touches.truncate(15);
            touches.push_front(HQMPuckTouch {
                player_index,
                skater_index,
                team,
                puck_pos,
                puck_speed,
                first_time: time,
                last_time: time,
            });
        }
    }
}

pub fn get_faceoff_positions(
    players: &HQMServerPlayerList,
    preferred_positions: &HashMap<HQMServerPlayerIndex, String>,
    world: &HQMGameWorld,
) -> HashMap<HQMServerPlayerIndex, (HQMTeam, String)> {
    let allowed_positions = &world.rink.allowed_positions;
    let mut res = HashMap::new();

    let mut red_players = smallvec::SmallVec::<[_; 32]>::new();
    let mut blue_players = smallvec::SmallVec::<[_; 32]>::new();
    for (player_index, player) in players.iter() {
        if let Some(player) = player {
            let team = player.object.map(|x| x.1);
            let i = match &player.data {
                HQMServerPlayerData::DualControl { movement, stick } => {
                    movement.or(*stick).unwrap_or(player_index)
                }
                _ => player_index,
            };
            let preferred_position = preferred_positions.get(&i).map(String::as_str);

            if team == Some(HQMTeam::Red) {
                red_players.push((player_index, preferred_position));
            } else if team == Some(HQMTeam::Blue) {
                blue_players.push((player_index, preferred_position));
            }
        }
    }

    setup_position(&mut res, &red_players, allowed_positions, HQMTeam::Red);
    setup_position(&mut res, &blue_players, allowed_positions, HQMTeam::Blue);

    res
}

pub fn has_players_in_offensive_zone(
    server: &HQMServer,
    team: HQMTeam,
    ignore_player: Option<HQMServerPlayerIndex>,
) -> bool {
    let line = match team {
        HQMTeam::Red => &server.game.world.rink.red_lines_and_net.offensive_line,
        HQMTeam::Blue => &server.game.world.rink.blue_lines_and_net.offensive_line,
    };

    for (player_index, player) in server.players.iter() {
        if let Some(player) = player {
            if let Some((object_index, skater_team)) = player.object {
                if skater_team == team && ignore_player != Some(player_index) {
                    if let Some(skater) = server.game.world.objects.get_skater(object_index) {
                        let feet_pos = &skater.body.pos
                            - (&skater.body.rot * Vector3::y().scale(skater.height));
                        let dot = (&feet_pos - &line.point).dot(&line.normal);
                        let leading_edge = -(line.width / 2.0);
                        if dot < leading_edge {
                            // Player is offside
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

fn setup_position(
    positions: &mut HashMap<HQMServerPlayerIndex, (HQMTeam, String)>,
    players: &[(HQMServerPlayerIndex, Option<&str>)],
    allowed_positions: &[String],
    team: HQMTeam,
) {
    let mut available_positions = Vec::from(allowed_positions);

    // First, we try to give each player its preferred position
    for (player_index, player_position) in players.iter() {
        if let Some(player_position) = player_position {
            if let Some(x) = available_positions
                .iter()
                .position(|x| x == *player_position)
            {
                let s = available_positions.remove(x);
                positions.insert(*player_index, (team, s));
            }
        }
    }

    // Some players did not get their preferred positions because they didn't have one,
    // or because it was already taken
    for (player_index, player_position) in players.iter() {
        if !positions.contains_key(player_index) {
            let s = if let Some(x) = available_positions.iter().position(|x| x == "C") {
                // Someone needs to be C
                let x = available_positions.remove(x);
                (team, x)
            } else if !available_positions.is_empty() {
                // Give out the remaining positions
                let x = available_positions.remove(0);
                (team, x)
            } else {
                // Oh no, we're out of legal starting positions
                if let Some(player_position) = player_position {
                    (team, (*player_position).to_owned())
                } else {
                    (team, "C".to_owned())
                }
            };
            positions.insert(*player_index, s);
        }
    }

    if let Some(x) = available_positions.iter().position(|x| x == "C") {
        let mut change_index = None;
        for (player_index, _) in players.iter() {
            if change_index.is_none() {
                change_index = Some(player_index);
            }

            if let Some((_, pos)) = positions.get(player_index) {
                if pos != "G" {
                    change_index = Some(player_index);
                    break;
                }
            }
        }

        if let Some(change_index) = change_index {
            let c = available_positions.remove(x);
            positions.insert(*change_index, (team, c));
        }
    }
}

pub fn find_empty_dual_control(
    server: &HQMServer,
    team: HQMTeam,
) -> Option<(
    HQMServerPlayerIndex,
    Option<HQMServerPlayerIndex>,
    Option<HQMServerPlayerIndex>,
)> {
    for (i, player) in server.players.iter() {
        if let Some(player) = player {
            if let HQMServerPlayerData::DualControl { movement, stick } = player.data {
                if movement.is_none() || stick.is_none() {
                    if let Some((_, dual_control_team)) = player.object {
                        if dual_control_team == team {
                            return Some((i, movement, stick));
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::hqm_behaviour_extra::setup_position;
    use crate::hqm_game::HQMTeam;
    use crate::hqm_server::HQMServerPlayerIndex;
    use std::collections::HashMap;

    #[test]
    fn test1() {
        let allowed_positions: Vec<String> = vec![
            "C", "LW", "RW", "LD", "RD", "G", "LM", "RM", "LLM", "RRM", "LLD", "RRD", "CM", "CD",
            "LW2", "RW2", "LLW", "RRW",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let c = "C";
        let lw = "LW";
        let rw = "RW";
        let g = "G";
        let mut res1 = HashMap::new();
        let players = vec![(HQMServerPlayerIndex(0), None)];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(HQMServerPlayerIndex(0), Some(c))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(HQMServerPlayerIndex(0), Some(lw))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![(HQMServerPlayerIndex(0), Some(g))];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![
            (HQMServerPlayerIndex(0usize), Some(c)),
            (HQMServerPlayerIndex(1), Some(lw)),
        ];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");
        assert_eq!(res1[&HQMServerPlayerIndex(1)].1, "LW");

        let mut res1 = HashMap::new();
        let players = vec![
            (HQMServerPlayerIndex(0), None),
            (HQMServerPlayerIndex(1), Some(lw)),
        ];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");
        assert_eq!(res1[&HQMServerPlayerIndex(1)].1, "LW");

        let mut res1 = HashMap::new();
        let players = vec![
            (HQMServerPlayerIndex(0), Some(rw)),
            (HQMServerPlayerIndex(1), Some(lw)),
        ];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");
        assert_eq!(res1[&HQMServerPlayerIndex(1)].1, "LW");

        let mut res1 = HashMap::new();
        let players = vec![
            (HQMServerPlayerIndex(0), Some(g)),
            (HQMServerPlayerIndex(1), Some(lw)),
        ];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "G");
        assert_eq!(res1[&HQMServerPlayerIndex(1)].1, "C");

        let mut res1 = HashMap::new();
        let players = vec![
            (HQMServerPlayerIndex(0usize), Some(c)),
            (HQMServerPlayerIndex(1), Some(c)),
        ];
        setup_position(
            &mut res1,
            players.as_ref(),
            &allowed_positions,
            HQMTeam::Red,
        );
        assert_eq!(res1[&HQMServerPlayerIndex(0)].1, "C");
        assert_eq!(res1[&HQMServerPlayerIndex(1)].1, "LW");
    }
}
