use crate::hqm_game::HQMPlayerInput;
use crate::hqm_server::HQMClientVersion;
use bytes::{BufMut, BytesMut};
use nalgebra::storage::Storage;
use nalgebra::{Matrix3, Vector2, Vector3, U1, U3};
use std::cmp::min;
use std::io::Error;

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
        player_name: Vec<u8>,
    },
    Update {
        current_game_id: u32,
        input: HQMPlayerInput,
        deltatime: Option<u32>,
        new_known_packet: u32,
        known_msg_pos: usize,
        chat: Option<(u8, Vec<u8>)>,
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
        let header = parser.read_bytes_aligned(4);
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
        let player_name = parser.read_bytes_aligned(32);
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
        let input = HQMPlayerInput {
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
                let message = parser.read_bytes_aligned(byte_num);
                Some((rep, message))
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
}

impl From<std::io::Error> for HQMClientToServerMessageDecoderError {
    fn from(value: Error) -> Self {
        HQMClientToServerMessageDecoderError::IoError(value)
    }
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
        self.align();
        self.buf.put_u8(v);
    }

    pub fn write_bytes_aligned(&mut self, v: &[u8]) {
        self.align();
        self.buf.put_slice(v);
    }

    pub fn write_bytes_aligned_padded(&mut self, n: usize, v: &[u8]) {
        self.align();
        let m = min(n, v.len());
        self.buf.put_slice(&v[0..m]);
        if n > m {
            self.buf.put_bytes(0, n - m);
        }
    }

    pub fn write_u32_aligned(&mut self, v: u32) {
        self.align();
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

    fn align(&mut self) {
        self.bit_pos = 0;
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
    pub(crate) pos: usize,
    pub(crate) bit_pos: u8,
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

    pub fn read_bytes_aligned(&mut self, n: usize) -> Vec<u8> {
        self.align();

        let mut res = Vec::with_capacity(n);
        for i in self.pos..(self.pos + n) {
            res.push(self.safe_get_byte(i))
        }
        self.pos = self.pos + n;
        return res;
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
