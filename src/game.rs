use crate::protocol;
use nalgebra::{point, Matrix3, Point3, Rotation3, Unit, Vector2, Vector3};

use crate::game::RinkSideOfLine::{BlueSide, On, RedSide};
use crate::protocol::{PuckPacket, SkaterPacket};
use arr_macro::arr;
use std::f32::consts::PI;
use std::fmt;
use std::fmt::{Display, Formatter};

/// Various time and scoreboard-related values.
///
/// These values are sent to the client and used to show period, time left, red and blue score,
/// the big "Game Over" text and offside and icing calls.
#[derive(Copy, Clone, Debug)]
pub struct ScoreboardValues {
    pub rules_state: RulesState,

    pub red_score: u32,
    pub blue_score: u32,
    pub period: u32,
    pub time: u32,
    pub goal_message_timer: u32,

    pub game_over: bool,
}

impl Default for ScoreboardValues {
    fn default() -> Self {
        ScoreboardValues {
            rules_state: RulesState::Regular {
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

/// Physics properties that are used for player and puck movement in the physics engine.
#[derive(Debug, Clone)]
pub struct PhysicsConfiguration {
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

impl Default for PhysicsConfiguration {
    fn default() -> Self {
        Self {
            gravity: 0.000680555,
            limit_jump_speed: false,
            player_acceleration: 0.000208333,
            player_deceleration: 0.000555555,
            max_player_speed: 0.05,
            puck_rink_friction: 0.05,
            player_turning: 0.00041666666,
            player_shift_acceleration: 0.00027777,
            max_player_shift_speed: 0.0333333,
            player_shift_turning: 0.00038888888,
        }
    }
}

/// Represents a line in the HQM rink.
#[derive(Debug, Clone)]
pub struct RinkLine {
    /// Z coordinate of middle of line.
    pub z: f32,
    /// Width of line.
    pub width: f32,
}

impl RinkLine {
    pub fn side_of_line(&self, pos: &Point3<f32>, radius: f32) -> RinkSideOfLine {
        let dot = pos.z - self.z;
        if dot > (self.width / 2.0) + radius {
            RedSide
        } else if dot < (-self.width) - radius {
            BlueSide
        } else {
            On
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RinkSideOfLine {
    BlueSide,
    On,
    RedSide,
}

/// A rink net.
#[derive(Debug, Clone)]
pub(crate) struct RinkNet {
    pub(crate) posts: Vec<(Point3<f32>, Point3<f32>, f32)>,
    pub(crate) surfaces: Vec<(Point3<f32>, Point3<f32>, Point3<f32>, Point3<f32>)>,
    pub(crate) left_post: Point3<f32>,
    pub(crate) right_post: Point3<f32>,
    pub(crate) normal: Vector3<f32>,
    pub(crate) left_post_inside: Vector3<f32>,
    pub(crate) right_post_inside: Vector3<f32>,
}

impl RinkNet {
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

        RinkNet {
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

/// A rink, with collision boundaries and nets.
///
/// In HQM, all coordinates are based in meters.
///
/// The X axis goes along the length of the rink,
/// which is 61 meters long by default. The coordinates starts with 0.0 on the wall closest to the blue net, a
/// and goes all the way up to 61.0 at the wall at the other end.
///
/// The Z axis goes along the width of the rink, which is 30 meters wide by default. The coordinates start with
/// 0.0 at the left wall looking from the position of the red goalie, and goes up to 30.0 at the right wall.
#[derive(Debug, Clone)]
pub struct Rink {
    pub(crate) planes: Vec<(Point3<f32>, Unit<Vector3<f32>>)>,
    pub(crate) corners: Vec<(Point3<f32>, Vector3<f32>, f32)>,
    pub(crate) red_net: RinkNet,
    pub(crate) blue_net: RinkNet,
    pub center_line: RinkLine,
    pub red_zone_blue_line: RinkLine,
    pub blue_zone_blue_line: RinkLine,
    pub width: f32,
    pub length: f32,
}

impl Rink {
    pub(crate) fn new(width: f32, length: f32, corner_radius: f32) -> Self {
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

        let blue_line_distance_neutral_zone_edge = 22.86;
        let blue_line_distance_mid = blue_line_distance_neutral_zone_edge - line_width / 2.0; // IIHF rule 17v and 17vi
                                                                                              // IIHF specifies distance between end boards and edge closest to the neutral zone, but my code specifies middle of line

        let center_x = width / 2.0;

        let red_zone_blueline_z = length - blue_line_distance_mid;
        let center_z = length / 2.0;
        let blue_zone_blueline_z = blue_line_distance_mid;

        let blue_net = RinkNet::new(
            Point3::new(center_x, 0.0, goal_line_distance),
            Matrix3::identity(),
        );
        let red_net = RinkNet::new(
            Point3::new(center_x, 0.0, length - goal_line_distance),
            Matrix3::from_columns(&[-Vector3::x(), Vector3::y(), -Vector3::z()]),
        );

        let red_zone_blue_line = RinkLine {
            z: red_zone_blueline_z,
            width: line_width,
        };
        let blue_zone_blue_line = RinkLine {
            z: blue_zone_blueline_z,
            width: line_width,
        };
        let center_line = RinkLine {
            z: center_z,
            width: line_width,
        };

        Rink {
            planes,
            corners,
            red_net,
            blue_net,
            center_line,
            red_zone_blue_line,
            blue_zone_blue_line,
            width,
            length,
        }
    }
}

/// Represents a physical body (both players and pucks) with a position, rotation and linear and angular velocities.
#[derive(Debug, Clone)]
pub struct PhysicsBody {
    pub pos: Point3<f32>,               // Measured in meters
    pub linear_velocity: Vector3<f32>,  // Measured in meters per hundred of a second
    pub rot: Rotation3<f32>,            // Rotation matrix
    pub angular_velocity: Vector3<f32>, // Measured in radians per hundred of a second
    pub(crate) rot_mul: Vector3<f32>,
}

/// Represents a skater object.
///
/// If you set the position, rotation, and/or linear velocity directly without adjusting the collision balls,
/// some weird things will happen with the inertia of the player. To fix this, use the reset_collision_balls method after
/// changing the physics properties.
#[derive(Debug, Clone)]
pub struct SkaterObject {
    pub body: PhysicsBody,
    /// Stick position in absolute space, measured in meters.
    pub stick_pos: Point3<f32>,
    /// Stick velocity, measured in meters per hundred of a second
    pub stick_velocity: Vector3<f32>,
    /// Stick rotation.
    pub stick_rot: Rotation3<f32>, // Rotation matrix
    /// Left-right body rotation around the Y axis in radians. Left is negative and right is positive, and the normal range is -(7/8)π to (7/8)π.
    pub head_rot: f32, // Radians
    /// Forward-backward body rotation around the X axis in radians. Backwards is negative and forwards is positive, and the normal range is -π/2 to π/2.
    pub body_rot: f32, // Radians
    pub(crate) height: f32,
    pub(crate) jumped_last_frame: bool,
    pub stick_placement: Vector2<f32>, // Azimuth and inclination in radians
    pub stick_placement_delta: Vector2<f32>, // Change in azimuth and inclination per hundred of a second
    pub collision_balls: Vec<SkaterCollisionBall>,
    pub hand: SkaterHand,
}

impl SkaterObject {
    pub fn new(pos: Point3<f32>, rot: Rotation3<f32>, hand: SkaterHand) -> Self {
        let linear_velocity = Vector3::new(0.0, 0.0, 0.0);
        let collision_balls = SkaterObject::get_collision_balls(&pos, &rot, &linear_velocity, 1.0);
        SkaterObject {
            body: PhysicsBody {
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
            jumped_last_frame: false,
            stick_placement: Vector2::new(0.0, 0.0),
            stick_placement_delta: Vector2::new(0.0, 0.0),
            hand,
            collision_balls,
        }
    }

    pub fn reset_collision_balls(&mut self) {
        self.collision_balls = Self::get_collision_balls(
            &self.body.pos,
            &self.body.rot,
            &self.body.linear_velocity,
            1.0,
        );
    }
    fn get_collision_balls(
        pos: &Point3<f32>,
        rot: &Rotation3<f32>,
        linear_velocity: &Vector3<f32>,
        mass: f32,
    ) -> Vec<SkaterCollisionBall> {
        let mut collision_balls = Vec::with_capacity(6);
        collision_balls.push(SkaterCollisionBall::from_skater(
            Vector3::new(0.0, 0.0, 0.0),
            pos,
            rot,
            linear_velocity,
            0.225,
            mass,
        ));
        collision_balls.push(SkaterCollisionBall::from_skater(
            Vector3::new(0.25, 0.3125, 0.0),
            pos,
            rot,
            linear_velocity,
            0.25,
            mass,
        ));
        collision_balls.push(SkaterCollisionBall::from_skater(
            Vector3::new(-0.25, 0.3125, 0.0),
            pos,
            rot,
            linear_velocity,
            0.25,
            mass,
        ));
        collision_balls.push(SkaterCollisionBall::from_skater(
            Vector3::new(-0.1875, -0.1875, 0.0),
            pos,
            rot,
            linear_velocity,
            0.1875,
            mass,
        ));
        collision_balls.push(SkaterCollisionBall::from_skater(
            Vector3::new(0.1875, -0.1875, 0.0),
            pos,
            rot,
            linear_velocity,
            0.1875,
            mass,
        ));
        collision_balls.push(SkaterCollisionBall::from_skater(
            Vector3::new(0.0, 0.5, 0.0),
            pos,
            &rot,
            linear_velocity,
            0.1875,
            mass,
        ));
        collision_balls
    }

    pub(crate) fn get_packet(&self) -> SkaterPacket {
        let rot = protocol::convert_matrix_to_network(31, &self.body.rot.matrix());
        let stick_rot = protocol::convert_matrix_to_network(25, &self.stick_rot.matrix());

        SkaterPacket {
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
pub struct SkaterCollisionBall {
    pub offset: Vector3<f32>,
    pub pos: Point3<f32>,
    pub velocity: Vector3<f32>,
    pub radius: f32,
    pub mass: f32,
}

impl SkaterCollisionBall {
    fn from_skater(
        offset: Vector3<f32>,
        skater_pos: &Point3<f32>,
        skater_rot: &Rotation3<f32>,
        velocity: &Vector3<f32>,
        radius: f32,
        mass: f32,
    ) -> Self {
        let pos = skater_pos + skater_rot * &offset;
        SkaterCollisionBall {
            offset,
            pos,
            velocity: velocity.clone_owned(),
            radius,
            mass,
        }
    }
}

/// Key and mouse inputs sent from the client to the server.
///
#[derive(Debug, Clone)]
pub struct PlayerInput {
    /// Stick angle. Normal range is -1 to 1.
    pub stick_angle: f32,
    /// Left or right turning. Negative is left, positive is right. Normal range is -1 to 1.
    pub turn: f32,
    /// Forward or backward movement. Negative is backwards, positive is forwards. Normal range is -1 to 1.
    pub fwbw: f32,
    /// Stick position.
    ///
    /// The position is a vector with a X axis and a Y axis value, both in radians.
    /// For the X axis, left is negative and right is positive, and the normal range is -π/2 to π/2.
    /// For the Y axis, down is negative and up is positive, and the normal range is -(5/16)π to π/8
    pub stick: Vector2<f32>,

    /// Left-right body rotation around the Y axis in radians. Left is negative and right is positive, and the normal range is -(7/8)π to (7/8)π.
    pub head_rot: f32,

    /// Forward-backward body rotation around the X axis in radians. Backwards is negative and forwards is positive, and the normal range is -π/2 to π/2.
    pub body_rot: f32,

    /// Key bit mask.
    /// Some utility methods are provided to check whether certain keys are pressed.
    pub keys: u32,
}

impl Default for PlayerInput {
    fn default() -> Self {
        PlayerInput {
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

impl PlayerInput {
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
pub enum SkaterHand {
    Left,
    Right,
}

/// Represents an HQM puck.
#[derive(Debug, Clone)]
pub struct PuckObject {
    pub body: PhysicsBody,
    pub radius: f32,
    pub height: f32,
}

impl PuckObject {
    pub fn new(pos: Point3<f32>, rot: Rotation3<f32>) -> Self {
        PuckObject {
            body: PhysicsBody {
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

    pub(crate) fn get_packet(&self) -> PuckPacket {
        let rot = protocol::convert_matrix_to_network(31, &self.body.rot.matrix());
        PuckPacket {
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

/// Rules state sent to the client.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RulesState {
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
pub struct PlayerIndex(pub(crate) usize);

impl std::fmt::Display for PlayerIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PlayerIndex {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(PlayerIndex)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Team {
    Red,
    Blue,
}

impl Team {
    pub(crate) fn get_num(self) -> u32 {
        match self {
            Team::Red => 0,
            Team::Blue => 1,
        }
    }

    pub fn get_other_team(self) -> Self {
        match self {
            Team::Red => Team::Blue,
            Team::Blue => Team::Red,
        }
    }
}

impl Display for Team {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Team::Red => write!(f, "Red"),
            Team::Blue => write!(f, "Blue"),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum PhysicsEvent {
    PuckTouch { player: PlayerIndex, puck: usize },
    PuckReachedDefensiveLine { team: Team, puck: usize },
    PuckPassedDefensiveLine { team: Team, puck: usize },
    PuckReachedCenterLine { team: Team, puck: usize },
    PuckPassedCenterLine { team: Team, puck: usize },
    PuckReachedOffensiveZone { team: Team, puck: usize },
    PuckEnteredOffensiveZone { team: Team, puck: usize },

    PuckEnteredNet { team: Team, puck: usize },
    PuckPassedGoalLine { team: Team, puck: usize },
    PuckTouchedNet { team: Team, puck: usize },
}
