use crate::hqm_parse;
use nalgebra::{Matrix3, Point3, Rotation3, Vector2, Vector3};
use std::borrow::Cow;
use std::fmt;
use std::fmt::{Display, Formatter};

use crate::hqm_server::{HQMObjectPacket, HQMPuckPacket, HQMSkaterPacket, ReplayTick};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;
use std::rc::Rc;
use std::time::Instant;

#[derive(Debug)]
pub struct HQMSkaterObjectRef<'a> {
    pub connected_player_index: usize,
    pub object_index: usize,
    pub team: HQMTeam,
    pub skater: &'a HQMSkater,
}

#[derive(Debug)]
pub struct HQMSkaterObjectRefMut<'a> {
    pub connected_player_index: usize,
    pub object_index: usize,
    pub team: HQMTeam,
    pub skater: &'a mut HQMSkater,
}

pub struct HQMGameWorld {
    pub objects: HQMGameWorldObjectList,
    puck_slots: usize,
    pub rink: HQMRink,
    pub physics_config: HQMPhysicsConfiguration,
}

pub struct HQMGameWorldObjectList {
    pub(crate) objects: Vec<HQMGameObject>,
}

impl HQMGameWorldObjectList {
    pub fn get_skater_object_for_player(
        &self,
        connected_player_index: usize,
    ) -> Option<HQMSkaterObjectRef> {
        for (object_index, object) in self.objects.iter().enumerate() {
            if let HQMGameObject::Player(player_index, team, skater) = object {
                if *player_index == connected_player_index {
                    return Some(HQMSkaterObjectRef {
                        connected_player_index,
                        object_index,
                        team: *team,
                        skater,
                    });
                }
            }
        }
        None
    }

    pub fn get_skater_object_for_player_mut(
        &mut self,
        connected_player_index: usize,
    ) -> Option<HQMSkaterObjectRefMut> {
        for (object_index, object) in self.objects.iter_mut().enumerate() {
            if let HQMGameObject::Player(player_index, team, skater) = object {
                if *player_index == connected_player_index {
                    return Some(HQMSkaterObjectRefMut {
                        connected_player_index,
                        object_index,
                        team: *team,
                        skater,
                    });
                }
            }
        }
        None
    }

    pub fn get_skater_iter(&self) -> impl Iterator<Item = HQMSkaterObjectRef> {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(object_index, obj)| {
                if let HQMGameObject::Player(connected_player_index, team, skater) = obj {
                    Some(HQMSkaterObjectRef {
                        connected_player_index: *connected_player_index,
                        object_index,
                        team: *team,
                        skater,
                    })
                } else {
                    None
                }
            })
    }

    pub fn get_skater_iter_mut(&mut self) -> impl Iterator<Item = HQMSkaterObjectRefMut> {
        self.objects
            .iter_mut()
            .enumerate()
            .filter_map(|(object_index, obj)| {
                if let HQMGameObject::Player(connected_player_index, team, skater) = obj {
                    Some(HQMSkaterObjectRefMut {
                        connected_player_index: *connected_player_index,
                        object_index,
                        team: *team,
                        skater,
                    })
                } else {
                    None
                }
            })
    }

    pub fn has_skater(&self, connected_player_index: usize) -> bool {
        for object in self.objects.iter() {
            if let HQMGameObject::Player(player_index, _, _) = object {
                if *player_index == connected_player_index {
                    return true;
                }
            }
        }
        false
    }

    pub fn get_puck(&self, object_index: usize) -> Option<&HQMPuck> {
        if let HQMGameObject::Puck(puck) = &self.objects[object_index] {
            Some(puck)
        } else {
            None
        }
    }

    pub fn get_puck_mut(&mut self, object_index: usize) -> Option<&mut HQMPuck> {
        if let HQMGameObject::Puck(puck) = &mut self.objects[object_index] {
            Some(puck)
        } else {
            None
        }
    }

    pub fn get_skater(&self, object_index: usize) -> Option<HQMSkaterObjectRef> {
        if let HQMGameObject::Player(connected_player_index, team, skater) =
            &self.objects[object_index]
        {
            Some(HQMSkaterObjectRef {
                connected_player_index: *connected_player_index,
                object_index,
                team: *team,
                skater,
            })
        } else {
            None
        }
    }

    pub fn get_skater_mut(&mut self, object_index: usize) -> Option<HQMSkaterObjectRefMut> {
        if let HQMGameObject::Player(connected_player_index, team, skater) =
            &mut self.objects[object_index]
        {
            Some(HQMSkaterObjectRefMut {
                connected_player_index: *connected_player_index,
                object_index,
                team: *team,
                skater,
            })
        } else {
            None
        }
    }
}

impl HQMGameWorld {
    pub(crate) fn get_internal_ref(
        &mut self,
        connected_player_index: usize,
    ) -> Option<(usize, &mut HQMGameObject)> {
        for (object_index, object) in self.objects.objects.iter_mut().enumerate() {
            if let HQMGameObject::Player(player_index, _, _) = object {
                if *player_index == connected_player_index {
                    return Some((object_index, object));
                }
            }
        }
        None
    }

    pub(crate) fn remove_player(&mut self, connected_player_index: usize) -> Option<usize> {
        let r = self.get_internal_ref(connected_player_index);
        if let Some((object_index, object)) = r {
            *object = HQMGameObject::None;
            return Some(object_index);
        }
        None
    }

    pub(crate) fn create_player_object(
        &mut self,
        team: HQMTeam,
        start: Point3<f32>,
        rot: Rotation3<f32>,
        hand: HQMSkaterHand,
        connected_player_index: usize,
        mass: f32,
    ) -> Option<usize> {
        let object_slot = self.find_empty_player_slot();
        if let Some(i) = object_slot {
            self.objects.objects[i] = HQMGameObject::Player(
                connected_player_index,
                team,
                HQMSkater::new(start, rot, hand, mass),
            );
        }
        return object_slot;
    }

    pub fn create_puck_object(&mut self, start: Point3<f32>, rot: Rotation3<f32>) -> Option<usize> {
        let object_slot = self.find_empty_puck_slot();
        if let Some(i) = object_slot {
            self.objects.objects[i] = HQMGameObject::Puck(HQMPuck::new(start, rot));
        }
        return object_slot;
    }

    fn find_empty_puck_slot(&self) -> Option<usize> {
        for i in 0..self.puck_slots {
            if let HQMGameObject::None = self.objects.objects[i] {
                return Some(i);
            }
        }
        None
    }

    fn find_empty_player_slot(&self) -> Option<usize> {
        for i in self.puck_slots..self.objects.objects.len() {
            if let HQMGameObject::None = self.objects.objects[i] {
                return Some(i);
            }
        }
        None
    }

    pub fn clear_pucks(&mut self) {
        for x in self.objects.objects[0..self.puck_slots].iter_mut() {
            *x = HQMGameObject::None;
        }
    }
}

pub struct HQMGame {
    pub(crate) start_time: DateTime<Utc>,

    pub(crate) persistent_messages: Vec<Rc<HQMMessage>>,
    pub(crate) replay_data: Vec<u8>,
    pub(crate) replay_msg_pos: usize,
    pub(crate) replay_last_packet: u32,
    pub(crate) replay_messages: Vec<Rc<HQMMessage>>,
    pub(crate) saved_packets: VecDeque<Vec<HQMObjectPacket>>,
    pub(crate) saved_pings: VecDeque<Instant>,
    pub(crate) saved_history: VecDeque<ReplayTick>,
    pub rules_state: HQMRulesState,
    pub world: HQMGameWorld,
    pub red_score: u32,
    pub blue_score: u32,
    pub period: u32,
    pub time: u32,
    pub time_break: u32,
    pub is_intermission_goal: bool,

    pub game_step: u32,
    pub game_over: bool,
    pub(crate) packet: u32,

    pub(crate) active: bool,

    pub history_length: usize,
}

#[derive(Debug, Clone)]
pub struct HQMPhysicsConfiguration {
    pub gravity: f32,
    pub limit_jump_speed: bool,
    pub player_acceleration: f32,
    pub player_deceleration: f32,
    pub max_player_speed: f32,
    pub puck_rink_friction: f32,
    pub player_turning: f32,
    pub player_shift_acceleration: f32,
    pub max_player_shift_speed: f32,
    pub player_shift_turning: f32,
}

impl HQMGame {
    pub fn new(
        puck_slots: usize,
        config: HQMPhysicsConfiguration,
        blue_line_location: f32,
    ) -> Self {
        let mut object_vec = Vec::with_capacity(32);
        for _ in 0..32 {
            object_vec.push(HQMGameObject::None);
        }
        let rink = HQMRink::new(30.0, 61.0, 8.5, blue_line_location);

        HQMGame {
            start_time: Utc::now(),

            persistent_messages: vec![],
            replay_data: Vec::with_capacity(64 * 1024 * 1024),
            replay_msg_pos: 0,
            replay_last_packet: u32::MAX,
            replay_messages: vec![],
            saved_packets: VecDeque::with_capacity(192),
            saved_pings: VecDeque::with_capacity(100),
            saved_history: VecDeque::new(),
            rules_state: HQMRulesState::Regular {
                offside_warning: false,
                icing_warning: false,
            },
            world: HQMGameWorld {
                objects: HQMGameWorldObjectList {
                    objects: object_vec,
                },
                puck_slots,
                rink,
                physics_config: config,
            },
            red_score: 0,
            blue_score: 0,
            period: 0,
            time: 30000,
            is_intermission_goal: false,
            time_break: 0,
            game_over: false,
            game_step: u32::MAX,
            packet: u32::MAX,
            active: false,
            history_length: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HQMRinkLine {
    pub point: Point3<f32>,
    pub width: f32,
    pub normal: Vector3<f32>,
}

impl HQMRinkLine {
    pub(crate) fn sphere_reached_line(&self, pos: &Point3<f32>, radius: f32) -> bool {
        let dot = (pos - &self.point).dot(&self.normal);
        let edge = self.width / 2.0;
        dot - radius < edge
    }

    pub(crate) fn sphere_past_leading_edge(&self, pos: &Point3<f32>, radius: f32) -> bool {
        let dot = (pos - &self.point).dot(&self.normal);
        let edge = -(self.width / 2.0);
        dot + radius < edge
    }

    pub fn point_past_middle_of_line(&self, pos: &Point3<f32>) -> bool {
        let dot = (pos - &self.point).dot(&self.normal);
        dot < 0.0
    }
}

#[derive(Debug, Clone)]
pub struct HQMRinkNet {
    pub(crate) posts: Vec<(Point3<f32>, Point3<f32>, f32)>,
    pub(crate) surfaces: Vec<(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>)>,
    pub(crate) left_post: Point3<f32>,
    pub(crate) right_post: Point3<f32>,
    pub(crate) normal: Vector3<f32>,
    pub(crate) left_post_inside: Vector3<f32>,
    pub(crate) right_post_inside: Vector3<f32>,
}

impl HQMRinkNet {
    fn new(pos: Point3<f32>, rot: Matrix3<f32>) -> Self {
        let front_width = 3.0;
        let back_width = 2.5;
        let front_half_width = front_width / 2.0;
        let back_half_width = back_width / 2.0;
        let height = 1.0;
        let upper_depth = 0.75;
        let lower_depth = 1.0;

        let (
            front_upper_left,
            front_upper_right,
            front_lower_left,
            front_lower_right,
            back_upper_left,
            back_upper_right,
            back_lower_left,
            back_lower_right,
        ) = (
            &pos + &rot * Vector3::new(-front_half_width, height, 0.0),
            &pos + &rot * Vector3::new(front_half_width, height, 0.0),
            &pos + &rot * Vector3::new(-front_half_width, 0.0, 0.0),
            &pos + &rot * Vector3::new(front_half_width, 0.0, 0.0),
            &pos + &rot * Vector3::new(-back_half_width, height, -upper_depth),
            &pos + &rot * Vector3::new(back_half_width, height, -upper_depth),
            &pos + &rot * Vector3::new(-back_half_width, 0.0, -lower_depth),
            &pos + &rot * Vector3::new(back_half_width, 0.0, -lower_depth),
        );

        HQMRinkNet {
            posts: vec![
                (front_lower_right.clone(), front_upper_right.clone(), 0.1875),
                (front_lower_left.clone(), front_upper_left.clone(), 0.1875),
                (front_upper_right.clone(), front_upper_left.clone(), 0.125),
                (front_lower_left.clone(), back_lower_left.clone(), 0.125),
                (front_lower_right.clone(), back_lower_right.clone(), 0.125),
                (front_upper_left.clone(), back_upper_left.clone(), 0.125),
                (back_upper_right.clone(), front_upper_right.clone(), 0.125),
                (back_lower_left.clone(), back_upper_left.clone(), 0.125),
                (back_lower_right.clone(), back_upper_right.clone(), 0.125),
                (back_lower_left.clone(), back_lower_right.clone(), 0.125),
                (back_upper_left.clone(), back_upper_right.clone(), 0.125),
            ],
            surfaces: vec![
                (
                    back_upper_left.clone(),
                    back_upper_right.clone(),
                    back_lower_right.clone(),
                    back_lower_left.clone(),
                ),
                (
                    front_upper_left.clone(),
                    back_upper_left.clone(),
                    back_lower_left.clone(),
                    front_lower_left.clone(),
                ),
                (
                    front_upper_right,
                    front_lower_right.clone(),
                    back_lower_right.clone(),
                    back_upper_right.clone(),
                ),
                (
                    front_upper_left.clone(),
                    front_upper_right.clone(),
                    back_upper_right.clone(),
                    back_upper_left.clone(),
                ),
            ],
            left_post: front_lower_left.clone(),
            right_post: front_lower_right.clone(),
            normal: rot * Vector3::z(),
            left_post_inside: &rot * Vector3::x(),
            right_post_inside: &rot * -Vector3::x(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinesAndNet {
    pub net: HQMRinkNet,
    pub mid_line: HQMRinkLine,
    pub offensive_line: HQMRinkLine,
    pub defensive_line: HQMRinkLine,
}

#[derive(Debug, Clone)]
pub struct HQMFaceoffSpot {
    pub center_position: Point3<f32>,
    pub red_player_positions: HashMap<String, (Point3<f32>, Rotation3<f32>)>,
    pub blue_player_positions: HashMap<String, (Point3<f32>, Rotation3<f32>)>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HQMRinkSide {
    Left,
    Right,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HQMRinkFaceoffSpot {
    Center,
    DefensiveZone(HQMTeam, HQMRinkSide),
    Offside(HQMTeam, HQMRinkSide),
}

#[derive(Debug, Clone)]
pub struct HQMRink {
    pub planes: Vec<(Point3<f32>, Vector3<f32>)>,
    pub corners: Vec<(Point3<f32>, Vector3<f32>, f32)>,
    pub red_lines_and_net: LinesAndNet,
    pub blue_lines_and_net: LinesAndNet,
    pub width: f32,
    pub length: f32,
    pub allowed_positions: Vec<String>,
    pub blue_zone_faceoff_spots: [HQMFaceoffSpot; 2],
    pub blue_neutral_faceoff_spots: [HQMFaceoffSpot; 2],
    pub center_faceoff_spot: HQMFaceoffSpot,
    pub red_neutral_faceoff_spots: [HQMFaceoffSpot; 2],
    pub red_zone_faceoff_spots: [HQMFaceoffSpot; 2],
}

impl HQMRink {
    fn new(width: f32, length: f32, corner_radius: f32, blue_line_distance: f32) -> Self {
        let zero = Point3::new(0.0, 0.0, 0.0);
        let planes = vec![
            (zero.clone(), Vector3::y()),
            (Point3::new(0.0, 0.0, length), -Vector3::z()),
            (zero.clone(), Vector3::z()),
            (Point3::new(width, 0.0, 0.0), -Vector3::x()),
            (zero.clone(), Vector3::x()),
        ];
        let r = corner_radius;
        let wr = width - corner_radius;
        let lr = length - corner_radius;
        let corners = vec![
            (
                Point3::new(r, 0.0, r),
                Vector3::new(-1.0, 0.0, -1.0),
                corner_radius,
            ),
            (
                Point3::new(wr, 0.0, r),
                Vector3::new(1.0, 0.0, -1.0),
                corner_radius,
            ),
            (
                Point3::new(wr, 0.0, lr),
                Vector3::new(1.0, 0.0, 1.0),
                corner_radius,
            ),
            (
                Point3::new(r, 0.0, lr),
                Vector3::new(-1.0, 0.0, 1.0),
                corner_radius,
            ),
        ];

        let line_width = 0.3; // IIHF rule 17iii, 17iv
        let goal_line_distance = 4.0; // IIHF rule 17iv

        let blue_line_distance_neutral_zone_edge = blue_line_distance;
        let blue_line_distance_mid = blue_line_distance_neutral_zone_edge - line_width / 2.0; // IIHF rule 17v and 17vi
                                                                                              // IIHF specifies distance between end boards and edge closest to the neutral zone, but my code specifies middle of line
        let distance_neutral_faceoff_spot = blue_line_distance_neutral_zone_edge + 1.5; // IIHF rule 18iv and 18vii
        let distance_zone_faceoff_spot = goal_line_distance + 6.0; // IIHF rule 18vi and 18vii

        let center_x = width / 2.0;
        let left_faceoff_x = center_x - 7.0; // IIHF rule 18vi and 18iv
        let right_faceoff_x = center_x + 7.0; // IIHF rule 18vi and 18iv

        let red_zone_faceoff_z = length - distance_zone_faceoff_spot;
        let red_zone_blueline_z = length - blue_line_distance_mid;
        let red_neutral_faceoff_z = length - distance_neutral_faceoff_spot;
        let center_z = length / 2.0;
        let blue_neutral_faceoff_z = distance_neutral_faceoff_spot;
        let blue_zone_blueline_z = blue_line_distance_mid;
        let blue_zone_faceoff_z = distance_zone_faceoff_spot;

        let red_line_normal = Vector3::z();
        let blue_line_normal = -Vector3::z();

        let red_net = HQMRinkNet::new(
            Point3::new(center_x, 0.0, goal_line_distance),
            Matrix3::identity(),
        );
        let blue_net = HQMRinkNet::new(
            Point3::new(center_x, 0.0, length - goal_line_distance),
            Matrix3::from_columns(&[-Vector3::x(), Vector3::y(), -Vector3::z()]),
        );
        let red_offensive_line = HQMRinkLine {
            point: Point3::new(0.0, 0.0, blue_zone_blueline_z),
            width: line_width,
            normal: red_line_normal.clone(),
        };
        let blue_offensive_line = HQMRinkLine {
            point: Point3::new(0.0, 0.0, red_zone_blueline_z),
            width: line_width,
            normal: blue_line_normal.clone(),
        };
        let red_defensive_line = HQMRinkLine {
            point: Point3::new(0.0, 0.0, red_zone_blueline_z),
            width: line_width,
            normal: red_line_normal.clone(),
        };
        let blue_defensive_line = HQMRinkLine {
            point: Point3::new(0.0, 0.0, blue_zone_blueline_z),
            width: line_width,
            normal: blue_line_normal.clone(),
        };
        let red_midline = HQMRinkLine {
            point: Point3::new(0.0, 0.0, center_z),
            width: line_width,
            normal: red_line_normal.clone(),
        };
        let blue_midline = HQMRinkLine {
            point: Point3::new(0.0, 0.0, center_z),
            width: line_width,
            normal: blue_line_normal.clone(),
        };

        let red_rot = Rotation3::identity();
        let blue_rot = Rotation3::from_euler_angles(0.0, PI, 0.0);
        let red_goalie_pos = Point3::new(width / 2.0, 1.5, length - 5.0);
        let blue_goalie_pos = Point3::new(width / 2.0, 1.5, 5.0);

        let create_faceoff_spot = |center_position: Point3<f32>| {
            let red_defensive_zone = center_position.z > length - 11.0;
            let blue_defensive_zone = center_position.z < 11.0;
            let (red_left, red_right) = if center_position.x < 9.0 {
                (true, false)
            } else if center_position.x > width - 9.0 {
                (false, true)
            } else {
                (false, false)
            };
            let blue_left = red_right;
            let blue_right = red_left;

            fn get_positions(
                center_position: &Point3<f32>,
                rot: &Rotation3<f32>,
                goalie_pos: &Point3<f32>,
                is_defensive_zone: bool,
                is_close_to_left: bool,
                is_close_to_right: bool,
            ) -> HashMap<String, (Point3<f32>, Rotation3<f32>)> {
                let mut player_positions = HashMap::new();

                let winger_z = 4.0;
                let m_z = 7.25;
                let d_z = if is_defensive_zone { 8.25 } else { 10.0 };
                let (far_left_winger_x, far_left_winger_z) = if is_close_to_left {
                    (-6.5, 3.0)
                } else {
                    (-10.0, winger_z)
                };
                let (far_right_winger_x, far_right_winger_z) = if is_close_to_right {
                    (6.5, 3.0)
                } else {
                    (10.0, winger_z)
                };

                let offsets = vec![
                    ("C", Vector3::new(0.0, 1.5, 2.75)),
                    ("LM", Vector3::new(-2.0, 1.5, m_z)),
                    ("RM", Vector3::new(2.0, 1.5, m_z)),
                    ("LW", Vector3::new(-5.0, 1.5, winger_z)),
                    ("RW", Vector3::new(5.0, 1.5, winger_z)),
                    ("LD", Vector3::new(-2.0, 1.5, d_z)),
                    ("RD", Vector3::new(2.0, 1.5, d_z)),
                    (
                        "LLM",
                        Vector3::new(
                            if is_close_to_left && is_defensive_zone {
                                -3.0
                            } else {
                                -5.0
                            },
                            1.5,
                            m_z,
                        ),
                    ),
                    (
                        "RRM",
                        Vector3::new(
                            if is_close_to_right && is_defensive_zone {
                                3.0
                            } else {
                                5.0
                            },
                            1.5,
                            m_z,
                        ),
                    ),
                    (
                        "LLD",
                        Vector3::new(
                            if is_close_to_left && is_defensive_zone {
                                -3.0
                            } else {
                                -5.0
                            },
                            1.5,
                            d_z,
                        ),
                    ),
                    (
                        "RRD",
                        Vector3::new(
                            if is_close_to_right && is_defensive_zone {
                                3.0
                            } else {
                                5.0
                            },
                            1.5,
                            d_z,
                        ),
                    ),
                    ("CM", Vector3::new(0.0, 1.5, m_z)),
                    ("CD", Vector3::new(0.0, 1.5, d_z)),
                    ("LW2", Vector3::new(-6.0, 1.5, winger_z)),
                    ("RW2", Vector3::new(6.0, 1.5, winger_z)),
                    (
                        "LLW",
                        Vector3::new(far_left_winger_x, 1.5, far_left_winger_z),
                    ),
                    (
                        "RRW",
                        Vector3::new(far_right_winger_x, 1.5, far_right_winger_z),
                    ),
                ];
                for (s, offset) in offsets {
                    let pos = center_position + rot * &offset;

                    player_positions.insert(String::from(s), (pos, rot.clone()));
                }

                player_positions.insert(String::from("G"), (goalie_pos.clone(), rot.clone()));

                player_positions
            }

            let red_player_positions = get_positions(
                &center_position,
                &red_rot,
                &red_goalie_pos,
                red_defensive_zone,
                red_left,
                red_right,
            );
            let blue_player_positions = get_positions(
                &center_position,
                &blue_rot,
                &blue_goalie_pos,
                blue_defensive_zone,
                blue_left,
                blue_right,
            );

            HQMFaceoffSpot {
                center_position,
                red_player_positions,
                blue_player_positions,
            }
        };

        HQMRink {
            planes,
            corners,
            red_lines_and_net: LinesAndNet {
                net: red_net,
                offensive_line: red_offensive_line,
                defensive_line: red_defensive_line,
                mid_line: red_midline,
            },
            blue_lines_and_net: LinesAndNet {
                net: blue_net,
                offensive_line: blue_offensive_line,
                defensive_line: blue_defensive_line,
                mid_line: blue_midline,
            },
            width,
            length,
            allowed_positions: vec![
                "C", "LW", "RW", "LD", "RD", "G", "LM", "RM", "LLM", "RRM", "LLD", "RRD", "CM",
                "CD", "LW2", "RW2", "LLW", "RRW",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            blue_zone_faceoff_spots: [
                create_faceoff_spot(Point3::new(left_faceoff_x, 0.0, blue_zone_faceoff_z)),
                create_faceoff_spot(Point3::new(right_faceoff_x, 0.0, blue_zone_faceoff_z)),
            ],
            blue_neutral_faceoff_spots: [
                create_faceoff_spot(Point3::new(left_faceoff_x, 0.0, blue_neutral_faceoff_z)),
                create_faceoff_spot(Point3::new(right_faceoff_x, 0.0, blue_neutral_faceoff_z)),
            ],
            center_faceoff_spot: create_faceoff_spot(Point3::new(center_x, 0.0, center_z)),
            red_neutral_faceoff_spots: [
                create_faceoff_spot(Point3::new(left_faceoff_x, 0.0, red_neutral_faceoff_z)),
                create_faceoff_spot(Point3::new(right_faceoff_x, 0.0, red_neutral_faceoff_z)),
            ],
            red_zone_faceoff_spots: [
                create_faceoff_spot(Point3::new(left_faceoff_x, 0.0, red_zone_faceoff_z)),
                create_faceoff_spot(Point3::new(right_faceoff_x, 0.0, red_zone_faceoff_z)),
            ],
        }
    }

    pub fn get_faceoff_spot(&self, spot: HQMRinkFaceoffSpot) -> &HQMFaceoffSpot {
        match spot {
            HQMRinkFaceoffSpot::Center => &self.center_faceoff_spot,
            HQMRinkFaceoffSpot::DefensiveZone(team, side) => {
                let faceoff_spots = match team {
                    HQMTeam::Red => &self.red_zone_faceoff_spots,
                    HQMTeam::Blue => &self.blue_zone_faceoff_spots,
                };
                let index = match side {
                    HQMRinkSide::Left => 0,
                    HQMRinkSide::Right => 1,
                };
                &faceoff_spots[index]
            }
            HQMRinkFaceoffSpot::Offside(team, side) => {
                let faceoff_spots = match team {
                    HQMTeam::Red => &self.red_neutral_faceoff_spots,
                    HQMTeam::Blue => &self.blue_neutral_faceoff_spots,
                };
                let index = match side {
                    HQMRinkSide::Left => 0,
                    HQMRinkSide::Right => 1,
                };
                &faceoff_spots[index]
            }
        }
    }

    pub fn get_offside_faceoff_spot(&self, pos: &Point3<f32>, team: HQMTeam) -> HQMRinkFaceoffSpot {
        let side = if pos.x <= self.width / 2.0 {
            HQMRinkSide::Left
        } else {
            HQMRinkSide::Right
        };
        let lines_and_net = match team {
            HQMTeam::Red => &self.red_lines_and_net,
            HQMTeam::Blue => &self.blue_lines_and_net,
        };
        if lines_and_net.offensive_line.point_past_middle_of_line(pos) {
            HQMRinkFaceoffSpot::Offside(team.get_other_team(), side)
        } else if lines_and_net.mid_line.point_past_middle_of_line(pos) {
            HQMRinkFaceoffSpot::Center
        } else if lines_and_net.defensive_line.point_past_middle_of_line(pos) {
            HQMRinkFaceoffSpot::Offside(team, side)
        } else {
            HQMRinkFaceoffSpot::DefensiveZone(team, side)
        }
    }

    pub fn get_icing_faceoff_spot(&self, pos: &Point3<f32>, team: HQMTeam) -> HQMRinkFaceoffSpot {
        let side = if pos.x <= self.width / 2.0 {
            HQMRinkSide::Left
        } else {
            HQMRinkSide::Right
        };

        HQMRinkFaceoffSpot::DefensiveZone(team, side)
    }
}

#[derive(Debug, Clone)]
pub struct HQMBody {
    pub pos: Point3<f32>,               // Measured in meters
    pub linear_velocity: Vector3<f32>,  // Measured in meters per hundred of a second
    pub rot: Rotation3<f32>,            // Rotation matrix
    pub angular_velocity: Vector3<f32>, // Measured in radians per hundred of a second
    pub(crate) rot_mul: Vector3<f32>,
}

#[derive(Debug, Clone)]
pub struct HQMSkater {
    pub body: HQMBody,
    pub stick_pos: Point3<f32>,       // Measured in meters
    pub stick_velocity: Vector3<f32>, // Measured in meters per hundred of a second
    pub stick_rot: Rotation3<f32>,    // Rotation matrix
    pub head_rot: f32,                // Radians
    pub body_rot: f32,                // Radians
    pub height: f32,
    pub input: HQMPlayerInput,
    pub jumped_last_frame: bool,
    pub stick_placement: Vector2<f32>, // Azimuth and inclination in radians
    pub stick_placement_delta: Vector2<f32>, // Change in azimuth and inclination per hundred of a second
    pub collision_balls: Vec<HQMSkaterCollisionBall>,
    pub hand: HQMSkaterHand,
}

impl HQMSkater {
    fn get_collision_balls(
        pos: &Point3<f32>,
        rot: &Rotation3<f32>,
        linear_velocity: &Vector3<f32>,
        mass: f32,
    ) -> Vec<HQMSkaterCollisionBall> {
        let mut collision_balls = Vec::with_capacity(6);
        collision_balls.push(HQMSkaterCollisionBall::from_skater(
            Vector3::new(0.0, 0.0, 0.0),
            pos,
            rot,
            linear_velocity,
            0.225,
            mass,
        ));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(
            Vector3::new(0.25, 0.3125, 0.0),
            pos,
            rot,
            linear_velocity,
            0.25,
            mass,
        ));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(
            Vector3::new(-0.25, 0.3125, 0.0),
            pos,
            rot,
            linear_velocity,
            0.25,
            mass,
        ));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(
            Vector3::new(-0.1875, -0.1875, 0.0),
            pos,
            rot,
            linear_velocity,
            0.1875,
            mass,
        ));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(
            Vector3::new(0.1875, -0.1875, 0.0),
            pos,
            rot,
            linear_velocity,
            0.1875,
            mass,
        ));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(
            Vector3::new(0.0, 0.5, 0.0),
            pos,
            &rot,
            linear_velocity,
            0.1875,
            mass,
        ));
        collision_balls
    }

    pub(crate) fn new(
        pos: Point3<f32>,
        rot: Rotation3<f32>,
        hand: HQMSkaterHand,
        mass: f32,
    ) -> Self {
        let linear_velocity = Vector3::new(0.0, 0.0, 0.0);
        let collision_balls = HQMSkater::get_collision_balls(&pos, &rot, &linear_velocity, mass);
        HQMSkater {
            body: HQMBody {
                pos: pos.clone(),
                linear_velocity,
                rot,
                angular_velocity: Vector3::new(0.0, 0.0, 0.0),
                rot_mul: Vector3::new(2.75, 6.16, 2.35),
            },
            stick_pos: pos.clone(),
            stick_velocity: Vector3::new(0.0, 0.0, 0.0),
            stick_rot: Rotation3::identity(),
            head_rot: 0.0,
            body_rot: 0.0,
            height: 0.75,
            input: HQMPlayerInput::default(),
            jumped_last_frame: false,
            stick_placement: Vector2::new(0.0, 0.0),
            stick_placement_delta: Vector2::new(0.0, 0.0),
            hand,
            collision_balls,
        }
    }

    pub(crate) fn get_packet(&self) -> HQMSkaterPacket {
        let rot = hqm_parse::convert_matrix_to_network(31, &self.body.rot.matrix());
        let stick_rot = hqm_parse::convert_matrix_to_network(25, &self.stick_rot.matrix());

        HQMSkaterPacket {
            pos: (
                get_position(17, 1024.0 * self.body.pos.x),
                get_position(17, 1024.0 * self.body.pos.y),
                get_position(17, 1024.0 * self.body.pos.z),
            ),
            rot,
            stick_pos: (
                get_position(13, 1024.0 * (self.stick_pos.x - self.body.pos.x + 4.0)),
                get_position(13, 1024.0 * (self.stick_pos.y - self.body.pos.y + 4.0)),
                get_position(13, 1024.0 * (self.stick_pos.z - self.body.pos.z + 4.0)),
            ),
            stick_rot,
            head_rot: get_position(16, (self.head_rot + 2.0) * 8192.0),
            body_rot: get_position(16, (self.body_rot + 2.0) * 8192.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HQMSkaterCollisionBall {
    pub offset: Vector3<f32>,
    pub pos: Point3<f32>,
    pub velocity: Vector3<f32>,
    pub radius: f32,
    pub mass: f32,
}

impl HQMSkaterCollisionBall {
    fn from_skater(
        offset: Vector3<f32>,
        skater_pos: &Point3<f32>,
        skater_rot: &Rotation3<f32>,
        velocity: &Vector3<f32>,
        radius: f32,
        mass: f32,
    ) -> Self {
        let pos = skater_pos + skater_rot * &offset;
        HQMSkaterCollisionBall {
            offset,
            pos,
            velocity: velocity.clone_owned(),
            radius,
            mass,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HQMPlayerInput {
    pub stick_angle: f32,
    pub turn: f32,
    pub fwbw: f32,
    pub stick: Vector2<f32>,
    pub head_rot: f32,
    pub body_rot: f32,
    pub keys: u32,
}

impl Default for HQMPlayerInput {
    fn default() -> Self {
        HQMPlayerInput {
            stick_angle: 0.0,
            turn: 0.0,
            fwbw: 0.0,
            stick: Vector2::new(0.0, 0.0),
            head_rot: 0.0,
            body_rot: 0.0,
            keys: 0,
        }
    }
}

impl HQMPlayerInput {
    pub fn jump(&self) -> bool {
        self.keys & 0x1 != 0
    }
    pub fn crouch(&self) -> bool {
        self.keys & 0x2 != 0
    }
    pub fn join_red(&self) -> bool {
        self.keys & 0x4 != 0
    }
    pub fn join_blue(&self) -> bool {
        self.keys & 0x8 != 0
    }
    pub fn shift(&self) -> bool {
        self.keys & 0x10 != 0
    }
    pub fn spectate(&self) -> bool {
        self.keys & 0x20 != 0
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HQMSkaterHand {
    Left,
    Right,
}
#[derive(Debug, Clone)]
pub struct HQMPuckTouch {
    pub player_index: usize,
    pub team: HQMTeam,
    pub puck_pos: Point3<f32>,
    pub puck_speed: f32,
    pub first_time: u32,
    pub last_time: u32,
}

#[derive(Debug, Clone)]
pub struct HQMPuck {
    pub body: HQMBody,
    pub radius: f32,
    pub height: f32,
    pub touches: VecDeque<HQMPuckTouch>,
}

impl HQMPuck {
    fn new(pos: Point3<f32>, rot: Rotation3<f32>) -> Self {
        HQMPuck {
            body: HQMBody {
                pos,
                linear_velocity: Vector3::new(0.0, 0.0, 0.0),
                rot,
                angular_velocity: Vector3::new(0.0, 0.0, 0.0),
                rot_mul: Vector3::new(223.5, 128.0, 223.5),
            },
            radius: 0.125,
            height: 0.0412500016391,
            touches: VecDeque::new(),
        }
    }

    pub(crate) fn get_packet(&self) -> HQMPuckPacket {
        let rot = hqm_parse::convert_matrix_to_network(31, &self.body.rot.matrix());
        HQMPuckPacket {
            pos: (
                get_position(17, 1024.0 * self.body.pos.x),
                get_position(17, 1024.0 * self.body.pos.y),
                get_position(17, 1024.0 * self.body.pos.z),
            ),
            rot,
        }
    }

    pub(crate) fn get_puck_vertices(&self) -> Vec<Point3<f32>> {
        let mut res = Vec::with_capacity(48);
        for i in 0..16 {
            let (sin, cos) = ((i as f32) * PI / 8.0).sin_cos();
            for j in -1..=1 {
                let point = Vector3::new(
                    cos * self.radius,
                    (j as f32) * self.height,
                    sin * self.radius,
                );
                let point2 = &self.body.rot * point;
                res.push(&self.body.pos + point2);
            }
        }
        res
    }

    pub fn add_touch(&mut self, player_index: usize, team: HQMTeam, time: u32) {
        let puck_pos = self.body.pos.clone();
        let puck_speed = self.body.linear_velocity.norm();
        let most_recent_touch = self.touches.front_mut();

        match most_recent_touch {
            Some(most_recent_touch)
                if most_recent_touch.player_index == player_index
                    && most_recent_touch.team == team =>
            {
                most_recent_touch.puck_pos = puck_pos;
                most_recent_touch.last_time = time;
                most_recent_touch.puck_speed = puck_speed;
            }
            _ => {
                self.touches.truncate(15);
                self.touches.push_front(HQMPuckTouch {
                    player_index,
                    team,
                    puck_pos,
                    puck_speed,
                    first_time: time,
                    last_time: time,
                });
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum HQMGameObject {
    None,
    Player(usize, HQMTeam, HQMSkater),
    Puck(HQMPuck),
}

#[derive(Debug, Clone)]
pub enum HQMMessage {
    PlayerUpdate {
        player_name: Rc<String>,
        object: Option<(usize, HQMTeam)>,
        player_index: usize,
        in_server: bool,
    },
    Goal {
        team: HQMTeam,
        goal_player_index: Option<usize>,
        assist_player_index: Option<usize>,
    },
    Chat {
        player_index: Option<usize>,
        message: Cow<'static, str>,
    },
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum HQMTeam {
    Red,
    Blue,
}

impl HQMTeam {
    pub(crate) fn get_num(self) -> u32 {
        match self {
            HQMTeam::Red => 0,
            HQMTeam::Blue => 1,
        }
    }

    pub fn get_other_team(self) -> Self {
        match self {
            HQMTeam::Red => HQMTeam::Blue,
            HQMTeam::Blue => HQMTeam::Red,
        }
    }
}

impl Display for HQMTeam {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HQMTeam::Red => write!(f, "Red"),
            HQMTeam::Blue => write!(f, "Blue"),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HQMRulesState {
    Regular {
        offside_warning: bool,
        icing_warning: bool,
    },
    Offside,
    Icing,
}

fn get_position(bits: u32, v: f32) -> u32 {
    let temp = v as i32;
    if temp < 0 {
        0
    } else if temp > ((1 << bits) - 1) {
        ((1 << bits) - 1) as u32
    } else {
        temp as u32
    }
}
