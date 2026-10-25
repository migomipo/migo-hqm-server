//! Faceoff placement and player-position assignment for standard matches.

use std::collections::HashMap;
use std::f32::consts::PI;

use glam::Vec3;
use glamx::Rot3;

use super::{ALLOWED_POSITIONS, Faceoff, RinkSide};
use crate::game::{PlayerId, Rink, Team};

pub(super) struct FaceoffPositions {
    pub(super) center: Vec3,
    pub(super) red: HashMap<&'static str, (Vec3, Rot3)>,
    pub(super) blue: HashMap<&'static str, (Vec3, Rot3)>,
}

pub(super) fn assign_team_positions(
    players: &[PlayerId],
    preferred: &HashMap<PlayerId, &'static str>,
) -> HashMap<PlayerId, &'static str> {
    let mut available = ALLOWED_POSITIONS.to_vec();
    let mut assignments = HashMap::new();

    // The first player requesting an available position claims it.
    for &id in players {
        if let Some(position) = preferred
            .get(&id)
            .copied()
            .and_then(|position| take_position(&mut available, position))
        {
            assignments.insert(id, position);
        }
    }

    // Fill the remaining slots, preferring center before the declared position order.
    for &id in players {
        assignments.entry(id).or_insert_with(|| {
            take_position(&mut available, "C")
                .or_else(|| take_next_position(&mut available))
                .unwrap_or_else(|| preferred.get(&id).copied().unwrap_or("C"))
        });
    }

    // If no one requested center, move the first non-goalie there.
    if available.contains(&"C")
        && let Some(&id) = players
            .iter()
            .find(|&&id| assignments[&id] != "G")
            .or_else(|| players.first())
    {
        assignments.insert(id, "C");
    }

    assignments
}

fn take_position(
    available: &mut Vec<&'static str>,
    requested: &'static str,
) -> Option<&'static str> {
    available
        .iter()
        .position(|&position| position == requested)
        .map(|index| available.remove(index))
}

fn take_next_position(available: &mut Vec<&'static str>) -> Option<&'static str> {
    (!available.is_empty()).then(|| available.remove(0))
}

pub(super) fn faceoff_spot(
    rink: &Rink,
    spot: Faceoff,
    spawn_offset: f32,
    altitude: f32,
) -> FaceoffPositions {
    let width = rink.width;
    let length = rink.length;
    let center_x = width / 2.0;
    let center = match spot {
        Faceoff::Center => Vec3::new(center_x, 0.0, length / 2.0),
        Faceoff::Defensive(team, side) => Vec3::new(
            if side == RinkSide::Lower {
                center_x - 7.0
            } else {
                center_x + 7.0
            },
            0.0,
            if team == Team::Red {
                length - 10.0
            } else {
                10.0
            },
        ),
        Faceoff::Offside(team, side) => Vec3::new(
            if side == RinkSide::Lower {
                center_x - 7.0
            } else {
                center_x + 7.0
            },
            0.0,
            if team == Team::Red {
                length - (rink.blue_zone_blue_line.z + 1.5)
            } else {
                rink.blue_zone_blue_line.z + 1.5
            },
        ),
    };
    let make_positions = |team: Team| {
        let rotation = if team == Team::Red {
            Rot3::IDENTITY
        } else {
            Rot3::from_rotation_y(PI)
        };
        let defensive = if team == Team::Red {
            center.z > length - 11.0
        } else {
            center.z < 11.0
        };
        let close_left = if team == Team::Red {
            center.x < 9.0
        } else {
            center.x > width - 9.0
        };
        let close_right = if team == Team::Red {
            center.x > width - 9.0
        } else {
            center.x < 9.0
        };
        let winger_z = 4.0;
        let middle_z = 7.25;
        let defense_z = if defensive { 8.25 } else { 10.0 };
        let far_left = if close_left {
            (-6.5, 3.0)
        } else {
            (-10.0, winger_z)
        };
        let far_right = if close_right {
            (6.5, 3.0)
        } else {
            (10.0, winger_z)
        };
        let offsets = [
            ("C", 0.0, spawn_offset),
            ("LM", -2.0, middle_z),
            ("RM", 2.0, middle_z),
            ("LW", -5.0, winger_z),
            ("RW", 5.0, winger_z),
            ("LD", -2.0, defense_z),
            ("RD", 2.0, defense_z),
            (
                "LLM",
                if close_left && defensive { -3.0 } else { -5.0 },
                middle_z,
            ),
            (
                "RRM",
                if close_right && defensive { 3.0 } else { 5.0 },
                middle_z,
            ),
            (
                "LLD",
                if close_left && defensive { -3.0 } else { -5.0 },
                defense_z,
            ),
            (
                "RRD",
                if close_right && defensive { 3.0 } else { 5.0 },
                defense_z,
            ),
            ("CM", 0.0, middle_z),
            ("CD", 0.0, defense_z),
            ("LW2", -6.0, winger_z),
            ("RW2", 6.0, winger_z),
            ("LLW", far_left.0, far_left.1),
            ("RRW", far_right.0, far_right.1),
        ];
        let mut positions = HashMap::new();
        for (name, x, z) in offsets {
            positions.insert(
                name,
                (center + rotation * Vec3::new(x, altitude, z), rotation),
            );
        }
        let goalie = if team == Team::Red {
            Vec3::new(center_x, altitude, length - 5.0)
        } else {
            Vec3::new(center_x, altitude, 5.0)
        };
        positions.insert("G", (goalie, rotation));
        positions
    };
    FaceoffPositions {
        center,
        red: make_positions(Team::Red),
        blue: make_positions(Team::Blue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::PlayerIndex;
    use std::collections::HashSet;

    const ALL_FACEOFFS: [Faceoff; 9] = [
        Faceoff::Center,
        Faceoff::Defensive(Team::Red, RinkSide::Lower),
        Faceoff::Defensive(Team::Red, RinkSide::Higher),
        Faceoff::Defensive(Team::Blue, RinkSide::Lower),
        Faceoff::Defensive(Team::Blue, RinkSide::Higher),
        Faceoff::Offside(Team::Red, RinkSide::Lower),
        Faceoff::Offside(Team::Red, RinkSide::Higher),
        Faceoff::Offside(Team::Blue, RinkSide::Lower),
        Faceoff::Offside(Team::Blue, RinkSide::Higher),
    ];

    fn players(count: usize) -> Vec<PlayerId> {
        (0..count)
            .map(|index| PlayerId {
                index: PlayerIndex(index),
                counter: 0,
            })
            .collect()
    }

    fn positions_of(
        players: &[PlayerId],
        assignments: &HashMap<PlayerId, &'static str>,
    ) -> Vec<&'static str> {
        players.iter().map(|id| assignments[id]).collect()
    }

    fn rink() -> Rink {
        Rink::new(30.0, 61.0, 8.5)
    }

    fn inside_rink(rink: &Rink, pos: Vec3) -> bool {
        let in_bounds = pos.x > 0.0 && pos.x < rink.width && pos.z > 0.0 && pos.z < rink.length;
        in_bounds
            && rink.corners.iter().all(|&(center, direction, radius)| {
                let x = pos.x - center.x;
                let z = pos.z - center.z;
                // Only points past the corner's center in both directions are in its rounded part
                x * direction.x <= 0.0 || z * direction.z <= 0.0 || x.hypot(z) < radius
            })
    }

    fn mirrored(rink: &Rink, pos: Vec3) -> Vec3 {
        Vec3::new(rink.width - pos.x, pos.y, rink.length - pos.z)
    }

    #[test]
    fn an_empty_team_gets_no_positions() {
        assert!(assign_team_positions(&[], &HashMap::new()).is_empty());
    }

    #[test]
    fn players_without_preferences_take_center_then_the_declared_order() {
        let team = players(4);
        let assignments = assign_team_positions(&team, &HashMap::new());

        assert_eq!(positions_of(&team, &assignments), ["C", "LW", "RW", "LD"]);
    }

    #[test]
    fn unclaimed_players_skip_positions_already_claimed() {
        let team = players(3);
        let preferred = HashMap::from([(team[2], "LW")]);
        let assignments = assign_team_positions(&team, &preferred);

        assert_eq!(positions_of(&team, &assignments), ["C", "RW", "LW"]);
    }

    #[test]
    fn a_full_team_gets_distinct_positions_even_when_preferences_collide() {
        let team = players(ALLOWED_POSITIONS.len());
        let preferred = team.iter().map(|&id| (id, "LW")).collect();
        let assignments = assign_team_positions(&team, &preferred);

        let distinct: HashSet<_> = assignments.values().collect();
        assert_eq!(distinct.len(), ALLOWED_POSITIONS.len());
        assert_eq!(assignments[&team[0]], "LW");
    }

    #[test]
    fn players_beyond_the_positions_fall_back_to_their_preference_or_center() {
        let team = players(ALLOWED_POSITIONS.len() + 2);
        let extra_goalie = team[ALLOWED_POSITIONS.len()];
        let extra_skater = team[ALLOWED_POSITIONS.len() + 1];
        let preferred = HashMap::from([(team[0], "G"), (extra_goalie, "G")]);
        let assignments = assign_team_positions(&team, &preferred);

        assert_eq!(assignments[&team[0]], "G");
        assert_eq!(assignments[&team[1]], "C");
        assert_eq!(assignments[&extra_goalie], "G");
        assert_eq!(assignments[&extra_skater], "C");
    }

    #[test]
    fn a_team_of_goalies_still_gets_a_center() {
        let team = players(2);
        let preferred = team.iter().map(|&id| (id, "G")).collect();
        let assignments = assign_team_positions(&team, &preferred);

        assert_eq!(positions_of(&team, &assignments), ["G", "C"]);
    }

    #[test]
    fn every_position_at_every_faceoff_is_inside_the_rink() {
        let rink = rink();
        for faceoff in ALL_FACEOFFS {
            let spot = faceoff_spot(&rink, faceoff, 2.75, 0.0);
            assert!(inside_rink(&rink, spot.center), "{faceoff:?}");

            for (team, positions) in [(Team::Red, &spot.red), (Team::Blue, &spot.blue)] {
                for name in ALLOWED_POSITIONS {
                    let (pos, _) = positions[name];
                    assert!(
                        inside_rink(&rink, pos),
                        "{team} {name} at {faceoff:?} is outside the rink: {pos}"
                    );
                }
            }
        }
    }

    #[test]
    fn faceoffs_are_mirror_images_for_the_two_teams() {
        let rink = rink();
        // Each faceoff and the one the other team gets in the same situation
        let pairs = [
            (Faceoff::Center, Faceoff::Center),
            (
                Faceoff::Defensive(Team::Red, RinkSide::Lower),
                Faceoff::Defensive(Team::Blue, RinkSide::Higher),
            ),
            (
                Faceoff::Defensive(Team::Red, RinkSide::Higher),
                Faceoff::Defensive(Team::Blue, RinkSide::Lower),
            ),
            (
                Faceoff::Offside(Team::Red, RinkSide::Lower),
                Faceoff::Offside(Team::Blue, RinkSide::Higher),
            ),
            (
                Faceoff::Offside(Team::Red, RinkSide::Higher),
                Faceoff::Offside(Team::Blue, RinkSide::Lower),
            ),
        ];

        for (faceoff, mirror) in pairs {
            let spot = faceoff_spot(&rink, faceoff, 2.75, 1.5);
            let mirror_spot = faceoff_spot(&rink, mirror, 2.75, 1.5);
            assert!(
                mirrored(&rink, spot.center).abs_diff_eq(mirror_spot.center, 1e-4),
                "{faceoff:?}"
            );

            for (positions, mirror_positions) in [
                (&spot.red, &mirror_spot.blue),
                (&spot.blue, &mirror_spot.red),
            ] {
                for name in ALLOWED_POSITIONS {
                    let (pos, rot) = positions[name];
                    let (mirror_pos, mirror_rot) = mirror_positions[name];
                    assert!(
                        mirrored(&rink, pos).abs_diff_eq(mirror_pos, 1e-4),
                        "{name} at {faceoff:?}: {pos} does not mirror {mirror_pos}"
                    );
                    let facing = rot * Vec3::Z;
                    let mirror_facing = mirror_rot * Vec3::Z;
                    assert!(
                        Vec3::new(-facing.x, facing.y, -facing.z).abs_diff_eq(mirror_facing, 1e-4),
                        "{name} at {faceoff:?} faces the wrong way"
                    );
                }
            }
        }
    }
}
