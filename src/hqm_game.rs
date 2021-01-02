use nalgebra::{Point3, Matrix3, Vector3, Vector2, Rotation3};
use std::fmt::{Display, Formatter};
use std::fmt;
use crate::hqm_parse;
use crate::hqm_parse::{HQMSkaterPacket, HQMPuckPacket};

use crate::hqm_server::HQMServerConfiguration;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;

pub(crate) struct HQMGameWorld {
    pub(crate) objects: Vec<HQMGameObject>,
    pub(crate) rink: HQMRink,
    pub(crate) gravity: f32,
    pub(crate) limit_jump_speed: bool,
}

impl HQMGameWorld {
    pub(crate) fn create_player_object (& mut self, team: HQMTeam, start: Point3<f32>, rot: Matrix3<f32>, hand: HQMSkaterHand,
                                        connected_player_index: usize, faceoff_position: String, mass: f32) -> Option<usize> {
        let object_slot = self.find_empty_object_slot();
        if let Some(i) = object_slot {
            self.objects[i] = HQMGameObject::Player(HQMSkater::new(i, team, start, rot, hand, connected_player_index, faceoff_position, mass));
        }
        return object_slot;
    }

    pub(crate) fn create_puck_object (& mut self, start: Point3<f32>, rot: Matrix3<f32>, cylinder_puck_post_collision: bool) -> Option<usize> {
        let object_slot = self.find_empty_object_slot();
        if let Some(i) = object_slot {
            self.objects[i] = HQMGameObject::Puck(HQMPuck::new(i, start, rot, cylinder_puck_post_collision));
        }
        return object_slot;
    }

    fn find_empty_object_slot(& self) -> Option<usize> {
        return self.objects.iter().position(|x| {match x {
            HQMGameObject::None  => true,
            _ => false
        }});
    }
}

#[derive(PartialEq, Debug, Clone)]
pub(crate) enum HQMIcingStatus {
    No,          // No icing
    NotTouched(Point3<f32>),  // Puck has entered offensive half, but not reached the goal line
    Warning(Point3<f32>),     // Puck has reached the goal line, delayed icing
    Icing        // Icing has been called
}

impl HQMIcingStatus {
    pub(crate) fn is_warning(&self) -> bool {
        match self {
            HQMIcingStatus::Warning(_) => true,
            _ => false
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
pub(crate) enum HQMOffsideStatus {
    No,                           // No offside
    InOffensiveZone,              // No offside, puck in offensive zone
    Warning(Point3<f32>, usize),  // Warning, puck entered offensive zone in an offside situation but not touched yet
    Offside                       // Offside has been called
}

impl HQMOffsideStatus {
    pub(crate) fn is_warning(&self) -> bool {
        match self {
            HQMOffsideStatus::Warning(_, _) => true,
            _ => false
        }
    }
}

pub(crate) struct HQMGame {

    pub(crate) state: HQMGameState,
    pub(crate) red_icing_status: HQMIcingStatus,
    pub(crate) blue_icing_status: HQMIcingStatus,
    pub(crate) red_offside_status: HQMOffsideStatus,
    pub(crate) blue_offside_status: HQMOffsideStatus,
    pub(crate) next_faceoff_spot: HQMFaceoffSpot,
    pub(crate) world: HQMGameWorld,
    pub(crate) red_score: u32,
    pub(crate) blue_score: u32,
    pub(crate) period: u32,
    pub(crate) time: u32,
    pub(crate) time_break: u32,
    pub(crate) is_intermission_goal: bool,
    pub(crate) paused: bool,
    pub(crate) game_id: u32,
    pub(crate) game_step: u32,
    pub(crate) game_over: bool,
    pub(crate) packet: u32,

    pub(crate) active: bool,
}

impl HQMGame {
    pub(crate) fn new (game_id: u32, config: &HQMServerConfiguration) -> Self {
        let mut object_vec = Vec::with_capacity(32);
        for _ in 0..32 {
            object_vec.push(HQMGameObject::None);
        }
        let rink = HQMRink::new(30.0, 61.0, 8.5);
        let mid_faceoff = rink.center_faceoff_spot.clone();

        HQMGame {
            state:HQMGameState::Warmup,
            red_icing_status: HQMIcingStatus::No,
            blue_icing_status: HQMIcingStatus::No,
            red_offside_status: HQMOffsideStatus::No,
            blue_offside_status: HQMOffsideStatus::No,
            next_faceoff_spot: mid_faceoff,
            world: HQMGameWorld {
                objects: object_vec,
                rink,
                gravity: 0.000680555,
                limit_jump_speed: config.limit_jump_speed
            },
            red_score: 0,
            blue_score: 0,
            period: 0,
            time: 30000,
            is_intermission_goal: false,
            time_break: 0,
            paused: false,

            game_over: false,
            game_id,
            game_step: u32::MAX,
            packet: u32::MAX,
            active: false,
        }
    }

    pub(crate) fn update_game_state(&mut self){
        self.state = if !self.paused {
            if self.game_over {
                HQMGameState::GameOver
            } else if self.time_break > 0 {
                if self.is_intermission_goal {
                    HQMGameState::GoalScored
                } else {
                    HQMGameState::Intermission
                }
            } else if self.period == 0 {
                HQMGameState::Warmup
            } else {
                HQMGameState::Game
            }
        } else {
            HQMGameState::Paused
        };
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HQMRinkLine {
    pub(crate) point: Point3<f32>,
    pub(crate) width: f32,
    pub(crate) normal: Vector3<f32>,
}

impl HQMRinkLine {

    pub(crate) fn sphere_reached_line (&self, pos: &Point3<f32>, radius: f32) -> bool {
        let dot = (pos - &self.point).dot (&self.normal);
        let edge = self.width / 2.0;
        dot - radius < edge
    }

    pub(crate) fn sphere_past_leading_edge (&self, pos: &Point3<f32>, radius: f32) -> bool {
        let dot = (pos - &self.point).dot (&self.normal);
        let edge = -(self.width / 2.0);
        dot + radius < edge
    }

    pub(crate) fn point_past_middle_of_line(&self, pos: &Point3<f32>) -> bool {
        let dot = (pos - &self.point).dot (&self.normal);
        dot < 0.0
    }

}


#[derive(Debug, Clone)]
pub(crate) struct HQMRinkNet {
    pub(crate) posts: Vec<(Point3<f32>, Point3<f32>, f32)>,
    pub(crate) surfaces: Vec<(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>)>,
    pub(crate) left_post: Point3<f32>,
    pub(crate) right_post: Point3<f32>,
    pub(crate) normal: Vector3<f32>,
    pub(crate) left_post_inside: Vector3<f32>,
    pub(crate) right_post_inside: Vector3<f32>
}

impl HQMRinkNet {
    fn new(pos: Point3<f32>, rot: Matrix3<f32>) -> Self {

        let (front_upper_left, front_upper_right, front_lower_left, front_lower_right,
            back_upper_left, back_upper_right, back_lower_left, back_lower_right) =
            (
                &pos + &rot * Vector3::new(-1.5, 1.0, 0.0),
                &pos + &rot * Vector3::new(1.5, 1.0, 0.0),
                &pos + &rot * Vector3::new(-1.5, 0.0, 0.0),
                &pos + &rot * Vector3::new(1.5, 0.0, 0.0),
                &pos + &rot * Vector3::new(-1.25, 1.0, -0.75),
                &pos + &rot * Vector3::new(1.25, 1.0, -0.75),
                &pos + &rot * Vector3::new(-1.25, 0.0, -1.0),
                &pos + &rot * Vector3::new(1.25, 0.0, -1.0)
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
                (back_upper_left.clone(), back_upper_right.clone(),
                 back_lower_right.clone(), back_lower_left.clone()),
                (front_upper_left.clone(), back_upper_left.clone(),
                 back_lower_left.clone(), front_lower_left.clone()),
                (front_upper_right, front_lower_right.clone(),
                 back_lower_right.clone(), back_upper_right.clone()),
                (front_upper_left.clone(), front_upper_right.clone(),
                 back_upper_right.clone(), back_upper_left.clone())
            ],
            left_post: front_lower_left.clone(),
            right_post: front_lower_right.clone(),
            normal: rot * Vector3::z(),
            left_post_inside: &rot * Vector3::x(),
            right_post_inside: &rot * -Vector3::x()
        }

    }
}

#[derive(Debug, Clone)]
pub(crate) struct LinesAndNet {
    pub(crate) net: HQMRinkNet,
    pub(crate) mid_line: HQMRinkLine,
    pub(crate) offensive_line: HQMRinkLine,
    pub(crate) defensive_line: HQMRinkLine
}

#[derive(Debug, Clone)]
pub(crate) struct HQMFaceoffSpot {
    pub(crate) center_position: Point3<f32>,
    pub(crate) red_player_positions: HashMap<String, (Point3<f32>, Rotation3<f32>)>,
    pub(crate) blue_player_positions: HashMap<String, (Point3<f32>, Rotation3<f32>)>
}

#[derive(Debug, Clone)]
pub(crate) struct HQMRink {
    pub(crate) planes: Vec<(Point3<f32>, Vector3<f32>)>,
    pub(crate) corners: Vec<(Point3<f32>, Vector3<f32>, f32)>,
    pub(crate) red_lines_and_net: LinesAndNet,
    pub(crate) blue_lines_and_net: LinesAndNet,
    pub(crate) width:f32,
    pub(crate) length:f32,
    pub(crate) allowed_positions: Vec<String>,
    pub(crate) blue_zone_faceoff_spots: [HQMFaceoffSpot; 2],
    pub(crate) blue_neutral_faceoff_spots: [HQMFaceoffSpot; 2],
    pub(crate) center_faceoff_spot: HQMFaceoffSpot,
    pub(crate) red_neutral_faceoff_spots: [HQMFaceoffSpot; 2],
    pub(crate) red_zone_faceoff_spots: [HQMFaceoffSpot; 2],
}

impl HQMRink {


    fn new(width: f32, length: f32, corner_radius: f32) -> Self {

        let zero = Point3::new(0.0,0.0,0.0);
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
            (Point3::new(r, 0.0, r),   Vector3::new(-1.0, 0.0, -1.0), corner_radius),
            (Point3::new(wr, 0.0, r),  Vector3::new( 1.0, 0.0, -1.0), corner_radius),
            (Point3::new(wr, 0.0, lr), Vector3::new( 1.0, 0.0,  1.0), corner_radius),
            (Point3::new(r, 0.0, lr),  Vector3::new(-1.0, 0.0,  1.0), corner_radius)
        ];

        let line_width = 0.3; // IIHF rule 17iii, 17iv
        let goal_line_distance = 4.0; // IIHF rule 17iv

        let blue_line_distance_neutral_zone_edge = 22.86;
        let blue_line_distance_mid = blue_line_distance_neutral_zone_edge - line_width/2.0; // IIHF rule 17v and 17vi
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

        let red_net = HQMRinkNet::new(Point3::new (center_x, 0.0, goal_line_distance), Matrix3::identity());
        let blue_net = HQMRinkNet::new(Point3::new (center_x, 0.0, length - goal_line_distance), Matrix3::from_columns (& [-Vector3::x(), Vector3::y(), -Vector3::z()]));
        let red_offensive_line = HQMRinkLine {
            point: Point3::new (0.0, 0.0, blue_zone_blueline_z),
            width: line_width,
            normal: red_line_normal.clone()
        };
        let blue_offensive_line = HQMRinkLine {
            point: Point3::new (0.0, 0.0, red_zone_blueline_z),
            width: line_width,
            normal: blue_line_normal.clone()
        };
        let red_defensive_line = HQMRinkLine {
            point: Point3::new (0.0, 0.0, red_zone_blueline_z),
            width: line_width,
            normal: red_line_normal.clone()
        };
        let blue_defensive_line = HQMRinkLine {
            point: Point3::new (0.0, 0.0, blue_zone_blueline_z),
            width: line_width,
            normal: blue_line_normal.clone()
        };
        let red_midline = HQMRinkLine {
            point: Point3::new (0.0, 0.0, center_z),
            width: line_width,
            normal: red_line_normal.clone()
        };
        let blue_midline = HQMRinkLine {
            point: Point3::new (0.0, 0.0, center_z),
            width: line_width,
            normal: blue_line_normal.clone()
        };

        HQMRink {
            planes,
            corners,
            red_lines_and_net: LinesAndNet {
                net: red_net,
                offensive_line: red_offensive_line,
                defensive_line: red_defensive_line,
                mid_line: red_midline
            },
            blue_lines_and_net: LinesAndNet {
                net: blue_net,
                offensive_line: blue_offensive_line,
                defensive_line: blue_defensive_line,
                mid_line: blue_midline
            },
            width,
            length,
            allowed_positions: vec!["C", "LW", "RW", "LD", "RD", "G"].into_iter().map(String::from).collect(),
            blue_zone_faceoff_spots: [
                create_faceoff_spot(Point3::new (left_faceoff_x, 0.0, blue_zone_faceoff_z), width, length),
                create_faceoff_spot(Point3::new (right_faceoff_x, 0.0, blue_zone_faceoff_z), width, length)
            ],
            blue_neutral_faceoff_spots: [
                create_faceoff_spot(Point3::new (left_faceoff_x, 0.0, blue_neutral_faceoff_z), width, length),
                create_faceoff_spot(Point3::new (right_faceoff_x, 0.0, blue_neutral_faceoff_z), width, length)
            ],
            center_faceoff_spot: create_faceoff_spot(Point3::new (center_x, 0.0, center_z), width, length),
            red_neutral_faceoff_spots: [
                create_faceoff_spot(Point3::new (left_faceoff_x, 0.0, red_neutral_faceoff_z), width, length),
                create_faceoff_spot(Point3::new (right_faceoff_x, 0.0, red_neutral_faceoff_z), width, length)
            ],
            red_zone_faceoff_spots: [
                create_faceoff_spot(Point3::new (left_faceoff_x, 0.0, red_zone_faceoff_z), width, length),
                create_faceoff_spot(Point3::new (right_faceoff_x, 0.0, red_zone_faceoff_z), width, length)
            ]
        }
    }

    pub fn get_offside_faceoff_spot(& self, pos: &Point3<f32>, team: HQMTeam) -> HQMFaceoffSpot {
        let left_side = if pos.x <= self.width/2.0 { 0usize } else { 1usize };
        let (lines_and_net, f1, f2, f3) = match team {
            HQMTeam::Red => {
                (& self.red_lines_and_net, &self.blue_neutral_faceoff_spots, &self.red_neutral_faceoff_spots, &self.red_zone_faceoff_spots)
            }
            HQMTeam::Blue => {
                (& self.blue_lines_and_net, &self.red_neutral_faceoff_spots, &self.blue_neutral_faceoff_spots, &self.blue_zone_faceoff_spots)
            }
        };
        if lines_and_net.offensive_line.point_past_middle_of_line(pos) {
            f1[left_side].clone()
        } else if lines_and_net.mid_line.point_past_middle_of_line(pos) {
            self.center_faceoff_spot.clone()
        } else if lines_and_net.defensive_line.point_past_middle_of_line(pos) {
            f2[left_side].clone()
        } else {
            f3[left_side].clone()
        }
    }

    pub fn get_icing_faceoff_spot(& self, pos: &Point3<f32>, team: HQMTeam) -> HQMFaceoffSpot {
        let left_side = if pos.x <= self.width/2.0 { 0usize } else { 1usize };
        match team {
            HQMTeam::Red => {
                self.red_zone_faceoff_spots[left_side].clone()
            }
            HQMTeam::Blue => {
                self.blue_zone_faceoff_spots[left_side].clone()
            },
        }
    }
}

fn create_faceoff_spot (center_position: Point3<f32>, rink_width: f32, rink_length: f32) -> HQMFaceoffSpot {
    let mut red_player_positions = HashMap::new();
    let mut blue_player_positions = HashMap::new();

    let offsets = vec![
        ("C", Vector3::new (0.0,1.5,2.75)),
        ("LD", Vector3::new (-2.0,1.5,7.25)),
            ("RD", Vector3::new (2.0,1.5,7.25)),
            ("LW", Vector3::new (-5.0,1.5,4.0)),
            ("RW", Vector3::new (5.0,1.5,4.0)),
    ];
    let red_rot = Rotation3::identity();
    let blue_rot = Rotation3::from_euler_angles(0.0, PI, 0.0);
    for (s, offset) in offsets {
        let red_pos = &center_position + &red_rot * &offset;
        let blue_pos = &center_position + &blue_rot * &offset;

        red_player_positions.insert(String::from (s), (red_pos, red_rot.clone()));
        blue_player_positions.insert( String::from (s), (blue_pos, blue_rot.clone()));
    }

    let red_goalie_pos = Point3::new (rink_width / 2.0, 1.5, rink_length - 5.0);
    let blue_goalie_pos = Point3::new (rink_width / 2.0, 1.5, 5.0);
    red_player_positions.insert(String::from ("G"), (red_goalie_pos, red_rot.clone()));
    blue_player_positions.insert(String::from ("G"), (blue_goalie_pos, blue_rot.clone()));
    HQMFaceoffSpot {
        center_position,
        red_player_positions,
        blue_player_positions
    }
}



#[derive(Debug, Clone)]
pub(crate) struct HQMBody {
    pub(crate) pos: Point3<f32>,                // Measured in meters
    pub(crate) linear_velocity: Vector3<f32>,   // Measured in meters per hundred of a second
    pub(crate) rot: Matrix3<f32>,               // Rotation matrix
    pub(crate) angular_velocity: Vector3<f32>,  // Measured in radians per hundred of a second
    pub(crate) rot_mul: Vector3<f32>
}

#[derive(Debug, Clone)]
pub(crate) struct HQMSkater {
    pub(crate) index: usize,
    pub(crate) connected_player_index: usize,
    pub(crate) body: HQMBody,
    pub(crate) team: HQMTeam,
    pub(crate) stick_pos: Point3<f32>,        // Measured in meters
    pub(crate) stick_velocity: Vector3<f32>,  // Measured in meters per hundred of a second
    pub(crate) stick_rot: Matrix3<f32>,       // Rotation matrix
    pub(crate) head_rot: f32,                 // Radians
    pub(crate) body_rot: f32,                 // Radians
    pub(crate) height: f32,
    pub(crate) input: HQMPlayerInput,
    pub(crate) jumped_last_frame: bool,
    pub(crate) stick_placement: Vector2<f32>,      // Azimuth and inclination in radians
    pub(crate) stick_placement_delta: Vector2<f32>, // Change in azimuth and inclination per hundred of a second
    pub(crate) collision_balls: Vec<HQMSkaterCollisionBall>,
    pub(crate) hand: HQMSkaterHand,
    pub(crate) faceoff_position: String
}

impl HQMSkater {

    fn get_collision_balls(pos: &Point3<f32>, rot: &Matrix3<f32>, linear_velocity: &Vector3<f32>, mass: f32) -> Vec<HQMSkaterCollisionBall> {
        let mut collision_balls = Vec::with_capacity(6);
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.0, 0.0, 0.0), pos, rot, linear_velocity, 0.225, mass));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.25, 0.3125, 0.0), pos, rot, linear_velocity, 0.25, mass));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(-0.25, 0.3125, 0.0), pos, rot, linear_velocity, 0.25, mass));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(-0.1875, -0.1875, 0.0), pos, rot, linear_velocity, 0.1875, mass));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.1875, -0.1875, 0.0), pos, rot, linear_velocity, 0.1875, mass));
        collision_balls.push(HQMSkaterCollisionBall::from_skater(Vector3::new(0.0, 0.5, 0.0), pos, & rot, linear_velocity, 0.1875, mass));
        collision_balls
    }

    fn new(object_index: usize, team: HQMTeam, pos: Point3<f32>, rot: Matrix3<f32>, hand: HQMSkaterHand, connected_player_index: usize, faceoff_position: String, mass: f32) -> Self {
        let linear_velocity = Vector3::new (0.0, 0.0, 0.0);
        let collision_balls = HQMSkater::get_collision_balls(&pos, &rot, &linear_velocity, mass);
        HQMSkater {
            index:object_index,
            connected_player_index,
            body: HQMBody {
                pos: pos.clone(),
                linear_velocity,
                rot,
                angular_velocity: Vector3::new (0.0, 0.0, 0.0),
                rot_mul: Vector3::new (2.75, 6.16, 2.35)
            },
            team,
            stick_pos: pos.clone(),
            stick_velocity: Vector3::new (0.0, 0.0, 0.0),
            stick_rot: Matrix3::identity(),
            head_rot: 0.0,
            body_rot: 0.0,
            height: 0.75,
            input: HQMPlayerInput::default(),
            jumped_last_frame: false,
            stick_placement: Vector2::new(0.0, 0.0),
            stick_placement_delta: Vector2::new(0.0, 0.0),
            hand,
            collision_balls,
            faceoff_position
        }
    }

    pub(crate) fn get_packet(&self) -> HQMSkaterPacket {
        let rot = hqm_parse::convert_matrix_to_network(31, & self.body.rot);
        let stick_rot = hqm_parse::convert_matrix_to_network(25, & self.stick_rot);

        HQMSkaterPacket {
            pos: (get_position (17, 1024.0 * self.body.pos.x),
                  get_position (17, 1024.0 * self.body.pos.y),
                  get_position (17, 1024.0 * self.body.pos.z)),
            rot,
            stick_pos: (get_position (13, 1024.0 * (self.stick_pos.x - self.body.pos.x + 4.0)),
                        get_position (13, 1024.0 * (self.stick_pos.y - self.body.pos.y + 4.0)),
                        get_position (13, 1024.0 * (self.stick_pos.z - self.body.pos.z + 4.0))),
            stick_rot,
            head_rot: get_position (16, (self.head_rot + 2.0) * 8192.0),
            body_rot: get_position (16, (self.body_rot + 2.0) * 8192.0)
        }
    }

}

#[derive(Debug, Clone)]
pub(crate) struct HQMSkaterCollisionBall {
    pub(crate) offset: Vector3<f32>,
    pub(crate) pos: Point3<f32>,
    pub(crate) velocity: Vector3<f32>,
    pub(crate) radius: f32,
    pub(crate) mass: f32

}

impl HQMSkaterCollisionBall {
    fn from_skater(offset: Vector3<f32>, skater_pos: & Point3<f32>, skater_rot: & Matrix3<f32>, velocity: & Vector3<f32>, radius: f32, mass: f32) -> Self {
        let pos = skater_pos + skater_rot * &offset;
        HQMSkaterCollisionBall {
            offset,
            pos,
            velocity: velocity.clone_owned(),
            radius,
            mass
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HQMPlayerInput {
    pub(crate) stick_angle: f32,
    pub(crate) turn: f32,
    pub(crate) unknown: f32,
    pub(crate) fwbw: f32,
    pub(crate) stick: Vector2<f32>,
    pub(crate) head_rot: f32,
    pub(crate) body_rot: f32,
    pub(crate) keys: u32,
}

impl Default for HQMPlayerInput {
    fn default() -> Self {
        HQMPlayerInput {
            stick_angle: 0.0,
            turn: 0.0,
            unknown: 0.0,
            fwbw: 0.0,
            stick: Vector2::new(0.0, 0.0),
            head_rot: 0.0,
            body_rot: 0.0,
            keys: 0
        }
    }
}

impl HQMPlayerInput {
    pub fn jump (&self) -> bool { self.keys & 0x1 != 0}
    pub fn crouch (&self) -> bool { self.keys & 0x2 != 0}
    pub fn join_red (&self) -> bool { self.keys & 0x4 != 0}
    pub fn join_blue (&self) -> bool { self.keys & 0x8 != 0}
    pub fn shift (&self) -> bool { self.keys & 0x10 != 0}
    pub fn spectate (&self) -> bool { self.keys & 0x20 != 0}
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum HQMSkaterHand {
    Left, Right
}
#[derive(Debug, Clone)]
pub(crate) struct HQMPuckTouch {
    pub(crate) player_index: usize,
    pub(crate) team: HQMTeam,
    pub(crate) puck_pos: Point3<f32>,
    pub(crate) time: u32,
    pub(crate) is_first_touch: bool
}

#[derive(Debug, Clone)]
pub(crate) struct HQMPuck {
    pub(crate) index: usize,
    pub(crate) body: HQMBody,
    pub(crate) radius: f32,
    pub(crate) height: f32,
    pub(crate) touches: VecDeque<HQMPuckTouch>,
    pub(crate) cylinder_puck_post_collision: bool
}

impl HQMPuck {
    fn new(object_index:usize, pos: Point3<f32>, rot: Matrix3<f32>, cylinder_puck_post_collision: bool) -> Self {
        HQMPuck {
            index:object_index,
            body: HQMBody {
                pos,
                linear_velocity: Vector3::new(0.0, 0.0, 0.0),
                rot,
                angular_velocity: Vector3::new(0.0,0.0,0.0),
                rot_mul: Vector3::new(223.5, 128.0, 223.5)
            },
            radius: 0.125,
            height: 0.0412500016391,
            touches: VecDeque::new(),
            cylinder_puck_post_collision
        }
    }

    pub(crate) fn get_packet(&self) -> HQMPuckPacket {
        let rot = hqm_parse::convert_matrix_to_network(31, & self.body.rot);
        HQMPuckPacket {
            pos: (get_position (17, 1024.0 * self.body.pos.x),
                  get_position (17, 1024.0 * self.body.pos.y),
                  get_position (17, 1024.0 * self.body.pos.z)),
            rot
        }
    }

    pub(crate) fn get_puck_vertices (&self) -> Vec<Point3<f32>> {
        let mut res = Vec::with_capacity(48);
        for i in 0..16 {

            let (sin, cos) = ((i as f32)*PI/8.0).sin_cos();
            for j in -1..=1 {
                let point = Vector3::new(cos * self.radius, (j as f32)*self.height, sin * self.radius);
                let point2 = &self.body.rot * point;
                res.push(&self.body.pos + point2);
            }
        }
        res
    }

    pub(crate) fn add_touch(& mut self, player_index: usize, team: HQMTeam, time: u32) {
        let puck_pos = self.body.pos.clone();
        let most_recent_touch = self.touches.front_mut();

        if let Some(most_recent_touch) = most_recent_touch {
            if most_recent_touch.player_index == player_index
                && most_recent_touch.team == team {
                most_recent_touch.puck_pos = puck_pos;
                most_recent_touch.time = time;
                most_recent_touch.is_first_touch = false;
            } else {
                self.touches.truncate(3);
                self.touches.push_front(HQMPuckTouch {
                    player_index,
                    team,
                    puck_pos,
                    time,
                    is_first_touch: true
                });
            }
        } else {
            self.touches.truncate(3);
            self.touches.push_front(HQMPuckTouch {
                player_index,
                team,
                puck_pos,
                time,
                is_first_touch: true
            });
        };


    }

}

#[derive(Debug, Clone)]
pub(crate) enum HQMGameObject {
    None,
    Player(HQMSkater),
    Puck(HQMPuck),
}



#[derive(Debug, Clone)]
pub(crate) enum HQMMessage {
    PlayerUpdate {
        player_name: String,
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
        message: String,
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
pub enum HQMGameState {
    Warmup,
    Game,
    Intermission,
    GoalScored,
    Paused,
    GameOver,
}

impl Display for HQMGameState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HQMGameState::Warmup => write!(f, "Warmup"),
            HQMGameState::Game => write!(f, "Game"),
            HQMGameState::Intermission => write!(f, "Intermission"),
            HQMGameState::GoalScored => write!(f, "Timeout"),
            HQMGameState::Paused => write!(f, "Paused"),
            HQMGameState::GameOver => write!(f, "Game Over"),

        }
    }
}


#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HQMRulesState {
    Regular {
        offside_warning: bool,
        icing_warning: bool
    },
    Offside,
    Icing,
}

fn get_position (bits: u32, v: f32) -> u32 {
    let temp = v as i32;
    if temp < 0 {
        0
    } else if temp > ((1 << bits) - 1) {
        ((1 << bits) - 1) as u32
    } else {
        temp as u32
    }
}