use crate::game::PlayerInput;
use crate::server::{HQMClientVersion, HQMMessage};
use arraydeque::{ArrayDeque, Wrapping};
use bytes::{BufMut, BytesMut};
use nalgebra::storage::Storage;
use nalgebra::{Matrix3, Vector2, Vector3, U1, U3};
use std::cmp::min;
use std::io::Error;
use std::string::FromUtf8Error;

const UXP: Vector3<f32> = Vector3::new(1.0, 0.0, 0.0);
const UXN: Vector3<f32> = Vector3::new(-1.0, 0.0, 0.0);
const UYP: Vector3<f32> = Vector3::new(0.0, 1.0, 0.0);
const UYN: Vector3<f32> = Vector3::new(0.0, -1.0, 0.0);
const UZP: Vector3<f32> = Vector3::new(0.0, 0.0, 1.0);
const UZN: Vector3<f32> = Vector3::new(0.0, 0.0, -1.0);

const TABLE: [[&'static Vector3<f32>; 3]; 8] = [
    [&UYP, &UXP, &UZP],
    [&UYP, &UZP, &UXN],
    [&UYP, &UZN, &UXP],
    [&UYP, &UXN, &UZN],
    [&UZP, &UXP, &UYN],
    [&UXN, &UZP, &UYN],
    [&UXP, &UZN, &UYN],
    [&UZN, &UXN, &UYN],
];

const GAME_HEADER: &[u8] = b"Hock";

pub enum HQMClientToServerMessage {
    Join {
        version: u32,
        player_name: String,
    },
    Update {
        current_game_id: u32,
        input: PlayerInput,
        deltatime: Option<u32>,
        new_known_packet: u32,
        known_msg_pos: usize,
        chat: Option<(u8, String)>,
        version: HQMClientVersion,
    },
    Exit,
    ServerInfo {
        version: u32,
        ping: u32,
    },
}

pub struct HQMMessageCodec;

impl HQMMessageCodec {
    pub fn parse_message(
        &self,
        src: &[u8],
    ) -> Result<HQMClientToServerMessage, HQMClientToServerMessageDecoderError> {
        let mut parser = HQMMessageReader::new(&src);
        let mut header = [0; 4];
        parser.read_bytes_aligned(&mut header);
        if header != GAME_HEADER {
            return Err(HQMClientToServerMessageDecoderError::WrongHeader);
        }

        let command = parser.read_byte_aligned();
        match command {
            0 => self.parse_request_info(&mut parser),
            2 => self.parse_player_join(&mut parser),
            4 => self.parse_player_update(&mut parser, HQMClientVersion::Vanilla),
            8 => self.parse_player_update(&mut parser, HQMClientVersion::Ping),
            0x10 => self.parse_player_update(&mut parser, HQMClientVersion::PingRules),
            7 => Ok(HQMClientToServerMessage::Exit),
            _ => Err(HQMClientToServerMessageDecoderError::UnknownType),
        }
    }

    fn parse_request_info(
        &self,
        parser: &mut HQMMessageReader,
    ) -> Result<HQMClientToServerMessage, HQMClientToServerMessageDecoderError> {
        let version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();
        Ok(HQMClientToServerMessage::ServerInfo { version, ping })
    }

    fn parse_player_join(
        &self,
        parser: &mut HQMMessageReader,
    ) -> Result<HQMClientToServerMessage, HQMClientToServerMessageDecoderError> {
        let version = parser.read_bits(8);
        let mut player_name = [0; 32];
        parser.read_bytes_aligned(&mut player_name);
        let player_name = get_player_name(&player_name)?;
        Ok(HQMClientToServerMessage::Join {
            version,
            player_name,
        })
    }

    fn parse_player_update(
        &self,
        parser: &mut HQMMessageReader,
        client_version: HQMClientVersion,
    ) -> Result<HQMClientToServerMessage, HQMClientToServerMessageDecoderError> {
        let current_game_id = parser.read_u32_aligned();

        let input_stick_angle = parser.read_f32_aligned();
        let input_turn = parser.read_f32_aligned();
        let _input_unknown = parser.read_f32_aligned();
        let input_fwbw = parser.read_f32_aligned();
        let input_stick_rot_1 = parser.read_f32_aligned();
        let input_stick_rot_2 = parser.read_f32_aligned();
        let input_head_rot = parser.read_f32_aligned();
        let input_body_rot = parser.read_f32_aligned();
        let input_keys = parser.read_u32_aligned();
        let input = PlayerInput {
            stick_angle: input_stick_angle,
            turn: input_turn,
            fwbw: input_fwbw,
            stick: Vector2::new(input_stick_rot_1, input_stick_rot_2),
            head_rot: input_head_rot,
            body_rot: input_body_rot,
            keys: input_keys,
        };

        let deltatime = if client_version.has_ping() {
            Some(parser.read_u32_aligned())
        } else {
            None
        };

        let new_known_packet = parser.read_u32_aligned();
        let known_msg_pos = parser.read_u16_aligned() as usize;

        let chat = {
            let has_chat_msg = parser.read_bits(1) == 1;
            if has_chat_msg {
                let rep = parser.read_bits(3) as u8;
                let byte_num = parser.read_bits(8) as usize;
                let mut bytes = [0; 256];

                parser.read_bytes_aligned(&mut bytes[0..byte_num]);
                let msg = String::from_utf8((&mut bytes[0..byte_num]).to_vec())?;
                Some((rep, msg))
            } else {
                None
            }
        };

        Ok(HQMClientToServerMessage::Update {
            current_game_id,
            input,
            deltatime,
            new_known_packet,
            known_msg_pos,
            chat,
            version: client_version,
        })
    }
}
pub enum HQMClientToServerMessageDecoderError {
    IoError(std::io::Error),
    WrongHeader,
    UnknownType,
    StringDecoding(FromUtf8Error),
}

impl From<std::io::Error> for HQMClientToServerMessageDecoderError {
    fn from(value: Error) -> Self {
        HQMClientToServerMessageDecoderError::IoError(value)
    }
}

impl From<FromUtf8Error> for HQMClientToServerMessageDecoderError {
    fn from(value: FromUtf8Error) -> Self {
        HQMClientToServerMessageDecoderError::StringDecoding(value)
    }
}

fn get_player_name(bytes: &[u8]) -> Result<String, FromUtf8Error> {
    let first_null = bytes.iter().position(|x| *x == 0);

    let bytes = match first_null {
        Some(x) => &bytes[0..x],
        None => &bytes[..],
    }
    .to_vec();
    let name = String::from_utf8(bytes)?;
    Ok(if name.is_empty() {
        "Noname".to_owned()
    } else {
        name
    })
}

pub fn convert_matrix_to_network(b: u8, v: &Matrix3<f32>) -> (u32, u32) {
    let r1 = convert_rot_column_to_network(b, &v.column(1));
    let r2 = convert_rot_column_to_network(b, &v.column(2));
    (r1, r2)
}

#[allow(dead_code)]
pub fn convert_matrix_from_network(b: u8, v1: u32, v2: u32) -> Matrix3<f32> {
    let r1 = convert_rot_column_from_network(b, v1);
    let r2 = convert_rot_column_from_network(b, v2);
    let r0 = r1.cross(&r2);
    Matrix3::from_columns(&[r0, r1, r2])
}

#[allow(dead_code)]
fn convert_rot_column_from_network(b: u8, v: u32) -> Vector3<f32> {
    let start = v & 7;

    let mut temp1 = TABLE[start as usize][0].clone();
    let mut temp2 = TABLE[start as usize][1].clone();
    let mut temp3 = TABLE[start as usize][2].clone();
    let mut pos = 3;
    while pos < b {
        let step = (v >> pos) & 3;
        let c1 = (temp1 + temp2).normalize();
        let c2 = (temp2 + temp3).normalize();
        let c3 = (temp1 + temp3).normalize();
        match step {
            0 => {
                temp2 = c1;
                temp3 = c3;
            }
            1 => {
                temp1 = c1;
                temp3 = c2;
            }
            2 => {
                temp1 = c3;
                temp2 = c2;
            }
            3 => {
                temp1 = c1;
                temp2 = c2;
                temp3 = c3;
            }
            _ => panic!(),
        }

        pos += 2;
    }
    (temp1 + temp2 + temp3).normalize()
}

fn convert_rot_column_to_network<S: Storage<f32, U3, U1>>(
    b: u8,
    v: &nalgebra::Matrix<f32, U3, U1, S>,
) -> u32 {
    let mut res = 0;

    if v[0] < 0.0 {
        res |= 1
    }
    if v[2] < 0.0 {
        res |= 2
    }
    if v[1] < 0.0 {
        res |= 4
    }
    let mut temp1 = TABLE[res as usize][0].clone();
    let mut temp2 = TABLE[res as usize][1].clone();
    let mut temp3 = TABLE[res as usize][2].clone();
    for i in (3..b).step_by(2) {
        let temp4 = (temp1 + temp2).normalize();
        let temp5 = (temp2 + temp3).normalize();
        let temp6 = (temp1 + temp3).normalize();

        let a1 = (temp4 - temp6).cross(&(v - temp6));
        if a1.dot(&v) < 0.0 {
            let a2 = (temp5 - temp4).cross(&(v - temp4));
            if a2.dot(&v) < 0.0 {
                let a3 = (temp6 - temp5).cross(&(v - temp5));
                if a3.dot(&v) < 0.0 {
                    res |= 3 << i;
                    temp1 = temp4;
                    temp2 = temp5;
                    temp3 = temp6;
                } else {
                    res |= 2 << i;
                    temp1 = temp6;
                    temp2 = temp5;
                }
            } else {
                res |= 1 << i;
                temp1 = temp4;
                temp3 = temp5;
            }
        } else {
            temp2 = temp4;
            temp3 = temp6;
        }
    }
    res
}

pub struct HQMMessageWriter<'a> {
    buf: &'a mut BytesMut,
    bit_pos: u8,
}

impl<'a> HQMMessageWriter<'a> {
    pub fn write_byte_aligned(&mut self, v: u8) {
        self.bit_pos = 0;
        self.buf.put_u8(v);
    }

    pub fn write_bytes_aligned(&mut self, v: &[u8]) {
        self.bit_pos = 0;
        self.buf.put_slice(v);
    }

    pub fn write_bytes_aligned_padded(&mut self, n: usize, v: &[u8]) {
        self.bit_pos = 0;
        let m = min(n, v.len());
        self.buf.put_slice(&v[0..m]);
        if n > m {
            self.buf.put_bytes(0, n - m);
        }
    }

    pub fn write_u32_aligned(&mut self, v: u32) {
        self.bit_pos = 0;
        self.buf.put_u32_le(v);
    }

    #[allow(dead_code)]
    pub fn write_f32_aligned(&mut self, v: f32) {
        self.write_u32_aligned(f32::to_bits(v));
    }

    pub fn write_pos(&mut self, n: u8, v: u32, old_v: Option<u32>) {
        let diff = match old_v {
            Some(old_v) => (v as i32) - (old_v as i32),
            None => i32::MAX,
        };
        if diff >= -(2 ^ 2) && diff <= 2 ^ 2 - 1 {
            self.write_bits(2, 0);
            self.write_bits(3, diff as u32);
        } else if diff >= -(2 ^ 5) && diff <= 2 ^ 5 - 1 {
            self.write_bits(2, 1);
            self.write_bits(6, diff as u32);
        } else if diff >= -(2 ^ 11) && diff <= 2 ^ 11 - 1 {
            self.write_bits(2, 2);
            self.write_bits(12, diff as u32);
        } else {
            self.write_bits(2, 3);
            self.write_bits(n, v);
        }
    }

    pub fn write_bits(&mut self, n: u8, v: u32) {
        let to_write = if n < 32 { !(u32::MAX << n) & v } else { v };
        let mut bits_remaining = n;
        let mut p = 0;
        while bits_remaining > 0 {
            let bits_possible_to_write = 8 - self.bit_pos;
            let bits = min(bits_remaining, bits_possible_to_write);
            let mask = !(u32::MAX << bits);
            let a = ((to_write >> p) & mask) as u8;

            if self.bit_pos == 0 {
                self.buf.put_u8(a);
            } else {
                *(self.buf.last_mut().unwrap()) |= a << self.bit_pos;
            }

            if bits_remaining >= bits_possible_to_write {
                bits_remaining -= bits_possible_to_write;
                self.bit_pos = 0;
                p += bits;
            } else {
                self.bit_pos += bits;
                bits_remaining = 0;
            }
        }
    }

    pub fn replay_fix(&mut self) {
        if self.bit_pos == 0 {
            self.buf.put_u8(0);
        }
    }

    pub fn new(buf: &'a mut BytesMut) -> Self {
        HQMMessageWriter { buf, bit_pos: 0 }
    }
}

pub struct HQMMessageReader<'a> {
    buf: &'a [u8],
    pos: usize,
    bit_pos: u8,
}

impl<'a> HQMMessageReader<'a> {
    #[allow(dead_code)]
    pub fn get_pos(&self) -> usize {
        self.pos
    }

    fn safe_get_byte(&self, pos: usize) -> u8 {
        if pos < self.buf.len() {
            self.buf[pos]
        } else {
            0
        }
    }

    pub fn read_byte_aligned(&mut self) -> u8 {
        self.align();
        let res = self.safe_get_byte(self.pos);
        self.pos = self.pos + 1;
        return res;
    }


    pub fn read_bytes_aligned(&mut self, out: &mut [u8]) {
        self.align();
        let n = out.len();

        for i in 0..n {
            out[i] = self.safe_get_byte(self.pos + i)
        }
        self.pos = self.pos + n;
    }

    pub fn read_u16_aligned(&mut self) -> u16 {
        self.align();
        let b1: u16 = self.safe_get_byte(self.pos).into();
        let b2: u16 = self.safe_get_byte(self.pos + 1).into();
        self.pos = self.pos + 2;
        return b1 | b2 << 8;
    }

    pub fn read_u32_aligned(&mut self) -> u32 {
        self.align();
        let b1: u32 = self.safe_get_byte(self.pos).into();
        let b2: u32 = self.safe_get_byte(self.pos + 1).into();
        let b3: u32 = self.safe_get_byte(self.pos + 2).into();
        let b4: u32 = self.safe_get_byte(self.pos + 3).into();
        self.pos = self.pos + 4;
        return b1 | b2 << 8 | b3 << 16 | b4 << 24;
    }

    pub fn read_f32_aligned(&mut self) -> f32 {
        let i = self.read_u32_aligned();
        return f32::from_bits(i);
    }

    #[allow(dead_code)]
    pub fn read_pos(&mut self, b: u8, old_value: Option<u32>) -> u32 {
        let pos_type = self.read_bits(2);
        match pos_type {
            0 => {
                let diff = self.read_bits_signed(3);
                let old_value = old_value.unwrap() as i32;
                (old_value + diff).max(0) as u32
            }
            1 => {
                let diff = self.read_bits_signed(6);
                let old_value = old_value.unwrap() as i32;
                (old_value + diff).max(0) as u32
            }
            2 => {
                let diff = self.read_bits_signed(12);
                let old_value = old_value.unwrap() as i32;
                (old_value + diff).max(0) as u32
            }
            3 => self.read_bits(b),
            _ => panic!(),
        }
    }

    #[allow(dead_code)]
    pub fn read_bits_signed(&mut self, b: u8) -> i32 {
        let a = self.read_bits(b);

        if a >= 1 << (b - 1) {
            (-1 << b) | (a as i32)
        } else {
            a as i32
        }
    }

    pub fn read_bits(&mut self, b: u8) -> u32 {
        let mut bits_remaining = b;
        let mut res = 0u32;
        let mut p = 0;
        while bits_remaining > 0 {
            let bits_possible_to_write = 8 - self.bit_pos;
            let bits = min(bits_remaining, bits_possible_to_write);

            let mask = if bits == 8 {
                u8::MAX
            } else {
                !(u8::MAX << bits)
            };
            let a = (self.safe_get_byte(self.pos) >> self.bit_pos) & mask;
            let a: u32 = a.into();
            res = res | (a << p);

            if bits_remaining >= bits_possible_to_write {
                bits_remaining -= bits_possible_to_write;
                self.bit_pos = 0;
                self.pos += 1;
                p += bits;
            } else {
                self.bit_pos += bits_remaining;
                bits_remaining = 0;
            }
        }
        return res;
    }

    pub fn align(&mut self) {
        if self.bit_pos > 0 {
            self.bit_pos = 0;
            self.pos += 1;
        }
    }

    #[allow(dead_code)]
    pub fn next(&mut self) {
        self.pos += 1;
        self.bit_pos = 0;
    }

    pub fn new(buf: &'a [u8]) -> Self {
        HQMMessageReader {
            buf,
            pos: 0,
            bit_pos: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ObjectPacket {
    None,
    Puck(PuckPacket),
    Skater(SkaterPacket),
}

#[derive(Debug, Clone)]
pub(crate) struct SkaterPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
    pub stick_pos: (u32, u32, u32),
    pub stick_rot: (u32, u32),
    pub head_rot: u32,
    pub body_rot: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct PuckPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
}

pub(crate) fn write_message(writer: &mut HQMMessageWriter, message: &HQMMessage) {
    match message {
        HQMMessage::Chat {
            player_index,
            message,
        } => {
            writer.write_bits(6, 2);
            writer.write_bits(
                6,
                match *player_index {
                    Some(x) => x.0 as u32,
                    None => u32::MAX,
                },
            );
            let message_bytes = message.as_bytes();
            let size = min(63, message_bytes.len());
            writer.write_bits(6, size as u32);

            for i in 0..size {
                writer.write_bits(7, message_bytes[i] as u32);
            }
        }
        HQMMessage::Goal {
            team,
            goal_player_index,
            assist_player_index,
        } => {
            writer.write_bits(6, 1);
            writer.write_bits(2, team.get_num());
            writer.write_bits(
                6,
                match *goal_player_index {
                    Some(x) => x.0 as u32,
                    None => u32::MAX,
                },
            );
            writer.write_bits(
                6,
                match *assist_player_index {
                    Some(x) => x.0 as u32,
                    None => u32::MAX,
                },
            );
        }
        HQMMessage::PlayerUpdate { player_index, data } => {
            writer.write_bits(6, 0);
            writer.write_bits(6, player_index.0 as u32);

            let (in_server, name_bytes) = match data {
                None => (false, &[] as &[u8]),
                Some(p) => (true, p.player_name.as_bytes()),
            };
            let (object_index, team_num) = match data.as_ref().and_then(|x| x.object) {
                Some((i, team)) => (i as u32, team.get_num()),
                None => (u32::MAX, u32::MAX),
            };
            writer.write_bits(1, if in_server { 1 } else { 0 });
            writer.write_bits(2, team_num);
            writer.write_bits(6, object_index);

            for i in 0usize..31 {
                let v = if i < name_bytes.len() {
                    name_bytes[i]
                } else {
                    0
                };
                writer.write_bits(7, v as u32);
            }
        }
    };
}

pub(crate) fn write_objects(
    writer: &mut HQMMessageWriter,
    packets: &ArrayDeque<[ObjectPacket; 32], 192, Wrapping>,
    current_packet: u32,
    known_packet: u32,
) {
    let current_packets = packets[0].as_slice();

    let old_packets = {
        let diff = if known_packet == u32::MAX {
            None
        } else {
            current_packet.checked_sub(known_packet)
        };
        if let Some(diff) = diff {
            let index = diff as usize;
            if index < 192 && index > 0 {
                packets.get(index)
            } else {
                None
            }
        } else {
            None
        }
    };

    writer.write_u32_aligned(current_packet);
    writer.write_u32_aligned(known_packet);

    for i in 0..32 {
        let current_packet = &current_packets[i];
        let old_packet = old_packets.map(|x| &x[i]);
        match current_packet {
            ObjectPacket::Puck(puck) => {
                let old_puck = old_packet.and_then(|x| match x {
                    ObjectPacket::Puck(old_puck) => Some(old_puck),
                    _ => None,
                });
                writer.write_bits(1, 1);
                writer.write_bits(2, 1); // Puck type
                writer.write_pos(17, puck.pos.0, old_puck.map(|puck| puck.pos.0));
                writer.write_pos(17, puck.pos.1, old_puck.map(|puck| puck.pos.1));
                writer.write_pos(17, puck.pos.2, old_puck.map(|puck| puck.pos.2));
                writer.write_pos(31, puck.rot.0, old_puck.map(|puck| puck.rot.0));
                writer.write_pos(31, puck.rot.1, old_puck.map(|puck| puck.rot.1));
            }
            ObjectPacket::Skater(skater) => {
                let old_skater = old_packet.and_then(|x| match x {
                    ObjectPacket::Skater(old_skater) => Some(old_skater),
                    _ => None,
                });
                writer.write_bits(1, 1);
                writer.write_bits(2, 0); // Skater type
                writer.write_pos(17, skater.pos.0, old_skater.map(|skater| skater.pos.0));
                writer.write_pos(17, skater.pos.1, old_skater.map(|skater| skater.pos.1));
                writer.write_pos(17, skater.pos.2, old_skater.map(|skater| skater.pos.2));
                writer.write_pos(31, skater.rot.0, old_skater.map(|skater| skater.rot.0));
                writer.write_pos(31, skater.rot.1, old_skater.map(|skater| skater.rot.1));
                writer.write_pos(
                    13,
                    skater.stick_pos.0,
                    old_skater.map(|skater| skater.stick_pos.0),
                );
                writer.write_pos(
                    13,
                    skater.stick_pos.1,
                    old_skater.map(|skater| skater.stick_pos.1),
                );
                writer.write_pos(
                    13,
                    skater.stick_pos.2,
                    old_skater.map(|skater| skater.stick_pos.2),
                );
                writer.write_pos(
                    25,
                    skater.stick_rot.0,
                    old_skater.map(|skater| skater.stick_rot.0),
                );
                writer.write_pos(
                    25,
                    skater.stick_rot.1,
                    old_skater.map(|skater| skater.stick_rot.1),
                );
                writer.write_pos(
                    16,
                    skater.head_rot,
                    old_skater.map(|skater| skater.head_rot),
                );
                writer.write_pos(
                    16,
                    skater.body_rot,
                    old_skater.map(|skater| skater.body_rot),
                );
            }
            ObjectPacket::None => {
                writer.write_bits(1, 0);
            }
        }
    }
}
