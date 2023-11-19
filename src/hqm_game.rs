use crate::hqm_parse;
use nalgebra::{point, Matrix3, Point3, Rotation3, Unit, Vector2, Vector3};

use std::fmt::Formatter;

use crate::hqm_parse::{HQMPuckPacket, HQMSkaterPacket};
use arr_macro::arr;
use std::f32::consts::PI;

pub struct HQMGameWorld {
    pub objects: HQMGameWorldObjectList,
    pub puck_slots: usize,
    pub rink: HQMRink,
    pub physics_config: HQMPhysicsConfiguration,
}

impl HQMGameWorld {
    pub(crate) fn new(
        puck_slots: usize,
        physics_config: HQMPhysicsConfiguration,
        blue_line_location: f32,
    ) -> Self {
        HQMGameWorld {
            objects: HQMGameWorldObjectList {
                objects: vec![HQMGameObject::None; 32],
            },
            puck_slots,
            rink: HQMRink::new(30.0, 61.0, 8.5, blue_line_location),
            physics_config,
        }
    }
}

pub struct HQMGameWorldObjectList {
    pub(crate) objects: Vec<HQMGameObject>,
}

impl HQMGameWorldObjectList {
    pub fn get_puck(&self, HQMObjectIndex(object_index): HQMObjectIndex) -> Option<&HQMPuck> {
        if let Some(HQMGameObject::Puck(puck)) = self.objects.get(object_index) {
            Some(puck)
        } else {
            None
        }
    }

    pub fn get_puck_mut(
        &mut self,
        HQMObjectIndex(object_index): HQMObjectIndex,
    ) -> Option<&mut HQMPuck> {
        if let Some(HQMGameObject::Puck(puck)) = self.objects.get_mut(object_index) {
            Some(puck)
        } else {
            None
        }
    }

    pub fn get_skater(&self, HQMObjectIndex(object_index): HQMObjectIndex) -> Option<&HQMSkater> {
        if let Some(HQMGameObject::Player(skater)) = self.objects.get(object_index) {
            Some(skater)
        } else {
            None
        }
    }

    pub fn get_skater_mut(
        &mut self,
        HQMObjectIndex(object_index): HQMObjectIndex,
    ) -> Option<&mut HQMSkater> {
        if let Some(HQMGameObject::Player(skater)) = self.objects.get_mut(object_index) {
            Some(skater)
        } else {
            None
        }
    }
}

impl HQMGameWorld {
    pub(crate) fn create_player_object(
        &mut self,
        start: Point3<f32>,
        rot: Rotation3<f32>,
        hand: HQMSkaterHand,
        mass: f32,
    ) -> Option<HQMObjectIndex> {
        let object_slot = self.find_empty_player_slot();
        if let Some(i) = object_slot {
            self.objects.objects[i.0] =
                HQMGameObject::Player(HQMSkater::new(start, rot, hand, mass));
        }
        return object_slot;
    }

    pub fn create_puck_object(
        &mut self,
        start: Point3<f32>,
        rot: Rotation3<f32>,
    ) -> Option<HQMObjectIndex> {
        let object_slot = self.find_empty_puck_slot();
        if let Some(i) = object_slot {
            self.objects.objects[i.0] = HQMGameObject::Puck(HQMPuck::new(start, rot));
        }
        return object_slot;
    }

    fn find_empty_puck_slot(&self) -> Option<HQMObjectIndex> {
        for i in 0..self.puck_slots {
            if let HQMGameObject::None = self.objects.objects[i] {
                return Some(HQMObjectIndex(i));
            }
        }
        None
    }

    fn find_empty_player_slot(&self) -> Option<HQMObjectIndex> {
        for i in self.puck_slots..self.objects.objects.len() {
            if let HQMGameObject::None = self.objects.objects[i] {
                return Some(HQMObjectIndex(i));
            }
        }
        None
    }

    pub fn clear_pucks(&mut self) {
        for x in self.objects.objects[0..self.puck_slots].iter_mut() {
            *x = HQMGameObject::None;
        }
    }

    pub(crate) fn remove_player(&mut self, HQMObjectIndex(i): HQMObjectIndex) -> bool {
        if let r @ HQMGameObject::Player(_) = &mut self.objects.objects[i] {
            *r = HQMGameObject::None;
            true
        } else {
            false
        }
    }

    pub fn remove_puck(&mut self, HQMObjectIndex(i): HQMObjectIndex) -> bool {
        if let r @ HQMGameObject::Puck(_) = &mut self.objects.objects[i] {
            *r = HQMGameObject::None;
            true
        } else {
            false
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HQMGameValues {
    pub rules_state: HQMRulesState,

    pub red_score: u32,
    pub blue_score: u32,
    pub period: u32,
    pub time: u32,
    pub goal_message_timer: u32,

    pub game_over: bool,
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
impl Default for HQMGameValues {
    fn default() -> Self {
        HQMGameValues {
            rules_state: HQMRulesState::Regular {
                offside_warning: false,
                icing_warning: false,
            },
            red_score: 0,
            blue_score: 0,
            period: 0,
            time: 30000,
            goal_message_timer: 0,
            game_over: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HQMRinkLine {
    pub point: Point3<f32>,
    pub width: f32,
    pub normal: Unit<Vector3<f32>>,
}

impl HQMRinkLine {
    pub(crate) fn sphere_reached_line(&self, pos: &Point3<f32>, radius: f32) -> bool {
        let dot = (pos - &self.point).dot(&self.normal);
        dot > -(self.width / 2.0) - radius
    }

    pub(crate) fn sphere_past_leading_edge(&self, pos: &Point3<f32>, radius: f32) -> bool {
        let dot = (pos - &self.point).dot(&self.normal);
        dot > (self.width / 2.0) + radius
    }

    pub fn point_past_middle_of_line(&self, pos: &Point3<f32>) -> bool {
        let dot = (pos - &self.point).dot(&self.normal);
        dot > 0.0
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
pub struct HQMRink {
    pub planes: Vec<(Point3<f32>, Unit<Vector3<f32>>)>,
    pub corners: Vec<(Point3<f32>, Vector3<f32>, f32)>,
    pub red_lines_and_net: LinesAndNet,
    pub blue_lines_and_net: LinesAndNet,
    pub width: f32,
    pub length: f32,
    pub blue_line_distance: f32,
}

impl HQMRink {
    fn new(width: f32, length: f32, corner_radius: f32, blue_line_distance: f32) -> Self {
        let zero = Point3::new(0.0, 0.0, 0.0);
        let planes = vec![
            (zero.clone(), Vector3::y_axis()),
            (Point3::new(0.0, 0.0, length), -Vector3::z_axis()),
            (zero.clone(), Vector3::z_axis()),
            (Point3::new(width, 0.0, 0.0), -Vector3::x_axis()),
            (zero.clone(), Vector3::x_axis()),
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

        let center_x = width / 2.0;

        let red_zone_blueline_z = length - blue_line_distance_mid;
        let center_z = length / 2.0;
        let blue_zone_blueline_z = blue_line_distance_mid;

        let red_line_normal = -Vector3::z_axis();
        let blue_line_normal = Vector3::z_axis();

        let blue_net = HQMRinkNet::new(
            Point3::new(center_x, 0.0, goal_line_distance),
            Matrix3::identity(),
        );
        let red_net = HQMRinkNet::new(
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
            blue_line_distance,
        }
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
pub struct HQMPuck {
    pub body: HQMBody,
    pub radius: f32,
    pub height: f32,
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

    pub(crate) fn get_puck_vertices(&self) -> [Point3<f32>; 48] {
        let mut res = arr![point![0.0, 0.0, 0.0]; 48];
        for i in 0..16 {
            let (sin, cos) = ((i as f32) * PI / 8.0).sin_cos();
            for j in -1..=1 {
                let point = Vector3::new(
                    cos * self.radius,
                    (j as f32) * self.height,
                    sin * self.radius,
                );
                let point2 = &self.body.rot * point;
                let index = i * 3 + 1 - j;
                res[index as usize] = self.body.pos + point2;
            }
        }
        res
    }
}

#[derive(Debug, Clone)]
pub(crate) enum HQMGameObject {
    None,
    Player(HQMSkater),
    Puck(HQMPuck),
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct HQMObjectIndex(pub usize);

impl std::fmt::Display for HQMObjectIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
