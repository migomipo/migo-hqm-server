use crate::game::{PlayerIndex, PlayerInput, RulesState, ScoreboardValues};
use crate::players::NetworkPlayerData;
use crate::server::{HQMClientVersion, HQMMessage, PacketHistory, PacketNumber};
use bytes::{BufMut, BytesMut};

use glam::{Vec2, Vec3};
use glamx::Rot3;
use std::cmp::min;
use std::io::Error;
use std::rc::Rc;
use std::string::FromUtf8Error;
use tokio_util::codec::{Decoder, Encoder};

const TABLE: [[Vec3; 3]; 8] = [
    [Vec3::Y, Vec3::X, Vec3::Z],
    [Vec3::Y, Vec3::Z, Vec3::NEG_X],
    [Vec3::Y, Vec3::NEG_Z, Vec3::X],
    [Vec3::Y, Vec3::NEG_X, Vec3::NEG_Z],
    [Vec3::Z, Vec3::X, Vec3::NEG_Y],
    [Vec3::NEG_X, Vec3::Z, Vec3::NEG_Y],
    [Vec3::X, Vec3::NEG_Z, Vec3::NEG_Y],
    [Vec3::NEG_Z, Vec3::NEG_X, Vec3::NEG_Y],
];

const GAME_HEADER: &[u8] = b"Hock";
const UPDATE_PACKET_TYPE: u8 = 5;
const NEW_GAME_PACKET_TYPE: u8 = 6;
const MAX_MESSAGES_PER_UPDATE: usize = 15;

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
    pub fn parse_message(&self, src: &[u8]) -> Result<HQMClientToServerMessage, HQMDecodeError> {
        let mut parser = HQMMessageReader::new(src);
        let mut header = [0; 4];
        parser.read_bytes_aligned(&mut header);
        if header != GAME_HEADER {
            return Err(HQMDecodeError::WrongHeader);
        }

        let command = parser.read_byte_aligned();
        match command {
            0 => self.parse_request_info(&mut parser),
            2 => self.parse_player_join(&mut parser),
            4 => self.parse_player_update(&mut parser, HQMClientVersion::Vanilla),
            8 => self.parse_player_update(&mut parser, HQMClientVersion::Ping),
            0x10 => self.parse_player_update(&mut parser, HQMClientVersion::PingRules),
            7 => Ok(HQMClientToServerMessage::Exit),
            _ => Err(HQMDecodeError::UnknownType),
        }
    }

    fn parse_request_info(
        &self,
        parser: &mut HQMMessageReader,
    ) -> Result<HQMClientToServerMessage, HQMDecodeError> {
        let version = parser.read_bits(8);
        let ping = parser.read_u32_aligned();
        Ok(HQMClientToServerMessage::ServerInfo { version, ping })
    }

    fn parse_player_join(
        &self,
        parser: &mut HQMMessageReader,
    ) -> Result<HQMClientToServerMessage, HQMDecodeError> {
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
    ) -> Result<HQMClientToServerMessage, HQMDecodeError> {
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
            stick: Vec2::new(input_stick_rot_1, input_stick_rot_2),
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
                let msg = String::from_utf8(bytes[0..byte_num].to_vec())?;
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

impl Decoder for HQMMessageCodec {
    type Item = HQMClientToServerMessage;
    type Error = HQMDecodeError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        let message = self.parse_message(src);
        src.clear();
        message.map(Some)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HQMServerUpdate<'a> {
    pub(crate) game_id: u32,
    pub(crate) history: &'a PacketHistory,
    pub(crate) game_step: u32,
    pub(crate) scoreboard: &'a ScoreboardValues,
    pub(crate) force_view: Option<PlayerIndex>,
}

pub(crate) enum HQMServerToClientMessage<'a> {
    ServerInfo {
        ping: u32,
        player_count: usize,
        team_size: u32,
        server_name: &'a str,
    },
    Update {
        update: HQMServerUpdate<'a>,
        recipient: &'a NetworkPlayerData,
    },
}

pub(crate) struct HQMMasterServerRegistration;

impl<'a> Encoder<HQMServerToClientMessage<'a>> for HQMMessageCodec {
    type Error = HQMEncodeError;

    fn encode(
        &mut self,
        message: HQMServerToClientMessage<'a>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        dst.clear();
        let mut writer = HQMMessageWriter::new(dst);

        match message {
            HQMServerToClientMessage::ServerInfo {
                ping,
                player_count,
                team_size,
                server_name,
            } => {
                writer.write_bytes_aligned(GAME_HEADER);
                writer.write_byte_aligned(1);
                writer.write_bits(8, 55);
                writer.write_u32_aligned(ping);
                writer.write_bits(8, player_count as u32);
                writer.write_bits(4, 4);
                writer.write_bits(4, team_size);
                writer.write_bytes_aligned_padded(32, server_name.as_bytes());
            }
            HQMServerToClientMessage::Update { update, recipient } => {
                write_update_packet(&mut writer, update, recipient);
            }
        }

        Ok(())
    }
}

impl Encoder<HQMMasterServerRegistration> for HQMMessageCodec {
    type Error = HQMEncodeError;

    fn encode(
        &mut self,
        _message: HQMMasterServerRegistration,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        dst.clear();
        dst.extend_from_slice(b"Hock\x20");
        Ok(())
    }
}

fn write_update_packet(
    mut writer: &mut HQMMessageWriter,
    update: HQMServerUpdate<'_>,
    player: &NetworkPlayerData,
) {
    if player.game_id != update.game_id {
        writer.write_bytes_aligned(GAME_HEADER);
        writer.write_byte_aligned(NEW_GAME_PACKET_TYPE);
        writer.write_u32_aligned(update.game_id);
        return;
    }

    writer.write_bytes_aligned(GAME_HEADER);
    writer.write_byte_aligned(UPDATE_PACKET_TYPE);
    writer.write_u32_aligned(update.game_id);
    writer.write_u32_aligned(update.game_step);
    writer.write_bits(1, u32::from(update.scoreboard.game_over));
    writer.write_bits(8, update.scoreboard.red_score);
    writer.write_bits(8, update.scoreboard.blue_score);
    writer.write_bits(16, update.scoreboard.time);

    writer.write_bits(16, update.scoreboard.goal_message_timer);
    writer.write_bits(8, update.scoreboard.period);
    let view = update.force_view.unwrap_or(player.view_player_index).0 as u32;
    writer.write_bits(8, view);

    if player.client_version.has_ping() {
        writer.write_u32_aligned(player.deltatime);
    }

    if player.client_version.has_rules() {
        let rules = match update.scoreboard.rules_state {
            RulesState::Regular {
                offside_warning,
                icing_warning,
            } => u32::from(offside_warning) | (u32::from(icing_warning) << 1),
            RulesState::Offside => 4,
            RulesState::Icing => 8,
        };
        writer.write_u32_aligned(rules);
    }

    write_objects(&mut writer, update.history, player.known_packet);

    let start = player.known_msgpos.min(player.messages.len());
    let remaining_messages = (player.messages.len() - start).min(MAX_MESSAGES_PER_UPDATE);
    writer.write_bits(4, remaining_messages as u32);
    writer.write_bits(16, start as u32);

    for message in &player.messages[start..start + remaining_messages] {
        write_message(&mut writer, Rc::as_ref(message));
    }
}

pub enum HQMDecodeError {
    IoError(std::io::Error),
    WrongHeader,
    UnknownType,
    StringDecoding(FromUtf8Error),
}

impl From<std::io::Error> for HQMDecodeError {
    fn from(value: Error) -> Self {
        HQMDecodeError::IoError(value)
    }
}

impl From<FromUtf8Error> for HQMDecodeError {
    fn from(value: FromUtf8Error) -> Self {
        HQMDecodeError::StringDecoding(value)
    }
}

pub enum HQMEncodeError {
    IoError(std::io::Error),
}

impl From<std::io::Error> for HQMEncodeError {
    fn from(value: Error) -> Self {
        HQMEncodeError::IoError(value)
    }
}

fn get_player_name(bytes: &[u8]) -> Result<String, FromUtf8Error> {
    let first_null = bytes.iter().position(|x| *x == 0);

    let bytes = match first_null {
        Some(x) => &bytes[0..x],
        None => bytes,
    }
    .to_vec();
    let name = String::from_utf8(bytes)?;
    Ok(if name.is_empty() {
        "Noname".to_owned()
    } else {
        name
    })
}

pub fn convert_matrix_to_network(b: u8, v: &Rot3) -> (u32, u32) {
    let r1 = convert_rot_column_to_network(b, v * Vec3::Y);
    let r2 = convert_rot_column_to_network(b, v * Vec3::Z);
    (r1, r2)
}

#[allow(dead_code)]
pub fn convert_matrix_from_network(b: u8, v1: u32, v2: u32) -> Rot3 {
    let r1 = convert_rot_column_from_network(b, v1);
    let r2 = convert_rot_column_from_network(b, v2);
    let r0 = r1.cross(r2);
    Rot3::from_rotation_axes(r0, r1, r2)
}

#[allow(dead_code)]
#[inline]
fn convert_rot_column_from_network(b: u8, v: u32) -> Vec3 {
    let start = v & 7;

    let [mut temp1, mut temp2, mut temp3] = TABLE[start as usize];
    let mut pos = 3;
    while pos < b {
        let step = (v >> pos) & 3;
        match step {
            0 => {
                temp2 = (temp1 + temp2).normalize();
                temp3 = (temp1 + temp3).normalize();
            }
            1 => {
                temp1 = (temp1 + temp2).normalize();
                temp3 = (temp2 + temp3).normalize();
            }
            2 => {
                temp1 = (temp1 + temp3).normalize();
                temp2 = (temp2 + temp3).normalize();
            }
            3 => {
                let c1 = (temp1 + temp2).normalize();
                let c2 = (temp2 + temp3).normalize();
                let c3 = (temp1 + temp3).normalize();
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

#[inline]
fn convert_rot_column_to_network(b: u8, v: Vec3) -> u32 {
    let oct = (v[0] < 0.0) as u32 | ((v[2] < 0.0) as u32) << 1 | ((v[1] < 0.0) as u32) << 2;

    let [mut t1, mut t2, mut t3] = TABLE[oct as usize];
    let mut res = oct;

    for i in (3..b).step_by(2) {
        let s4 = t1 + t2;
        let s5 = t2 + t3;
        let s6 = t1 + t3;

        // The normalization factors are positive, so they do not affect the
        // signs used to select a child triangle.
        if s6.cross(s4).dot(v) < 0.0 {
            if s4.cross(s5).dot(v) < 0.0 {
                if s5.cross(s6).dot(v) < 0.0 {
                    res |= 3 << i;
                    t1 = s4.normalize();
                    t2 = s5.normalize();
                    t3 = s6.normalize();
                } else {
                    res |= 2 << i;
                    t1 = s6.normalize();
                    t2 = s5.normalize();
                }
            } else {
                res |= 1 << i;
                t1 = s4.normalize();
                t3 = s5.normalize();
            }
        } else {
            t2 = s4.normalize();
            t3 = s6.normalize();
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

        if (-2i32.pow(2)..=2i32.pow(2) - 1).contains(&diff) {
            self.write_bits(2, 0);
            self.write_bits(3, diff as u32);
        } else if (-2i32.pow(5)..=2i32.pow(5) - 1).contains(&diff) {
            self.write_bits(2, 1);
            self.write_bits(6, diff as u32);
        } else if (-2i32.pow(11)..=2i32.pow(11) - 1).contains(&diff) {
            self.write_bits(2, 2);
            self.write_bits(12, diff as u32);
        } else {
            self.write_bits(2, 3);
            self.write_bits(n, v);
        }
    }

    pub fn write_bits(&mut self, n: u8, v: u32) {
        debug_assert!(n <= 32);
        let v = if n < 32 { v & !(u32::MAX << n) } else { v };
        let staged: u64 = (v as u64) << self.bit_pos;
        let total_bits = self.bit_pos + n;
        let total_bytes = (total_bits + 7) / 8;

        if self.bit_pos > 0 {
            *self.buf.last_mut().unwrap() |= staged as u8;
        }
        let start = if self.bit_pos > 0 { 1 } else { 0 };
        for i in start..total_bytes as usize {
            self.buf.put_u8((staged >> (i * 8)) as u8);
        }

        self.bit_pos = total_bits % 8;
    }

    pub fn recording_fix(&mut self) {
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
        self.pos += 1;
        res
    }

    pub fn read_bytes_aligned(&mut self, out: &mut [u8]) {
        self.align();
        let n = out.len();

        for i in 0..n {
            out[i] = self.safe_get_byte(self.pos + i)
        }
        self.pos += n;
    }

    pub fn read_u16_aligned(&mut self) -> u16 {
        self.align();
        let bytes = [
            self.safe_get_byte(self.pos),
            self.safe_get_byte(self.pos + 1),
        ];
        self.pos += 2;
        u16::from_le_bytes(bytes)
    }

    pub fn read_u32_aligned(&mut self) -> u32 {
        self.align();
        let bytes = [
            self.safe_get_byte(self.pos),
            self.safe_get_byte(self.pos + 1),
            self.safe_get_byte(self.pos + 2),
            self.safe_get_byte(self.pos + 3),
        ];
        self.pos += 4;
        u32::from_le_bytes(bytes)
    }

    pub fn read_f32_aligned(&mut self) -> f32 {
        let i = self.read_u32_aligned();
        f32::from_bits(i)
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
        debug_assert!(b <= 32);

        let total_bytes = ((self.bit_pos + b + 7) / 8) as usize;

        let mut staged = 0u64;
        for i in 0..total_bytes {
            let byte = self.buf.get(self.pos + i).copied().unwrap_or(0);
            staged |= (byte as u64) << (i * 8);
        }

        staged >>= self.bit_pos;
        let res = if b < 32 {
            (staged as u32) & !(u32::MAX << b)
        } else {
            staged as u32
        };

        self.bit_pos += b;
        self.pos += (self.bit_pos / 8) as usize;
        self.bit_pos %= 8;

        res
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Team;
    use std::borrow::Cow;
    use std::f32::consts::PI;

    fn write(f: impl FnOnce(&mut HQMMessageWriter)) -> Vec<u8> {
        let mut buf = BytesMut::new();
        f(&mut HQMMessageWriter::new(&mut buf));
        buf.to_vec()
    }

    fn mask(width: u8) -> u32 {
        if width == 32 {
            u32::MAX
        } else {
            (1 << width) - 1
        }
    }

    fn parse(packet: &[u8]) -> HQMClientToServerMessage {
        match HQMMessageCodec.parse_message(packet) {
            Ok(message) => message,
            Err(_) => panic!("packet should parse"),
        }
    }

    fn client_packet(command: u8, body: impl FnOnce(&mut HQMMessageWriter)) -> Vec<u8> {
        write(|w| {
            w.write_bytes_aligned(GAME_HEADER);
            w.write_byte_aligned(command);
            body(w);
        })
    }

    fn update_packet(
        command: u8,
        deltatime: Option<u32>,
        chat: impl FnOnce(&mut HQMMessageWriter),
    ) -> Vec<u8> {
        client_packet(command, |w| {
            w.write_u32_aligned(7); // Game ID
            // Stick angle, turn, unused, forward/back, stick x/y, head and body rotation
            for value in [0.5, -1.0, 99.0, 0.25, 0.1, -0.2, 1.5, -1.5] {
                w.write_f32_aligned(value);
            }
            w.write_u32_aligned(0b1010); // Keys
            if let Some(deltatime) = deltatime {
                w.write_u32_aligned(deltatime);
            }
            w.write_u32_aligned(321); // Known packet
            w.write_bytes_aligned(&17u16.to_le_bytes()); // Known message position
            chat(w);
        })
    }

    fn no_chat(w: &mut HQMMessageWriter) {
        w.write_bits(1, 0);
    }

    fn with_chat(rep: u32, length: usize, text: &[u8]) -> impl FnOnce(&mut HQMMessageWriter) {
        move |w| {
            w.write_bits(1, 1);
            w.write_bits(3, rep);
            w.write_bits(8, length as u32);
            w.write_bytes_aligned(text);
        }
    }

    fn read_objects(
        reader: &mut HQMMessageReader,
        baseline: Option<&[ObjectPacket; 32]>,
    ) -> Vec<ObjectPacket> {
        (0..32)
            .map(|i| {
                if reader.read_bits(1) == 0 {
                    return ObjectPacket::None;
                }
                let old = baseline.map(|objects| &objects[i]);
                match reader.read_bits(2) {
                    0 => {
                        let old = match old {
                            Some(ObjectPacket::Skater(skater)) => Some(skater),
                            _ => None,
                        };
                        ObjectPacket::Skater(SkaterPacket {
                            pos: (
                                reader.read_pos(17, old.map(|s| s.pos.0)),
                                reader.read_pos(17, old.map(|s| s.pos.1)),
                                reader.read_pos(17, old.map(|s| s.pos.2)),
                            ),
                            rot: (
                                reader.read_pos(31, old.map(|s| s.rot.0)),
                                reader.read_pos(31, old.map(|s| s.rot.1)),
                            ),
                            stick_pos: (
                                reader.read_pos(13, old.map(|s| s.stick_pos.0)),
                                reader.read_pos(13, old.map(|s| s.stick_pos.1)),
                                reader.read_pos(13, old.map(|s| s.stick_pos.2)),
                            ),
                            stick_rot: (
                                reader.read_pos(25, old.map(|s| s.stick_rot.0)),
                                reader.read_pos(25, old.map(|s| s.stick_rot.1)),
                            ),
                            head_rot: reader.read_pos(16, old.map(|s| s.head_rot)),
                            body_rot: reader.read_pos(16, old.map(|s| s.body_rot)),
                        })
                    }
                    1 => {
                        let old = match old {
                            Some(ObjectPacket::Puck(puck)) => Some(puck),
                            _ => None,
                        };
                        ObjectPacket::Puck(PuckPacket {
                            pos: (
                                reader.read_pos(17, old.map(|p| p.pos.0)),
                                reader.read_pos(17, old.map(|p| p.pos.1)),
                                reader.read_pos(17, old.map(|p| p.pos.2)),
                            ),
                            rot: (
                                reader.read_pos(31, old.map(|p| p.rot.0)),
                                reader.read_pos(31, old.map(|p| p.rot.1)),
                            ),
                        })
                    }
                    object_type => panic!("unknown object type {object_type}"),
                }
            })
            .collect()
    }

    fn objects(f: impl FnOnce(&mut [ObjectPacket; 32])) -> [ObjectPacket; 32] {
        let mut objects = std::array::from_fn(|_| ObjectPacket::None);
        f(&mut objects);
        objects
    }

    fn puck(x: u32) -> ObjectPacket {
        ObjectPacket::Puck(PuckPacket {
            pos: (x, 200, x + 300),
            rot: (x * 7, x * 11),
        })
    }

    fn skater(x: u32) -> ObjectPacket {
        ObjectPacket::Skater(SkaterPacket {
            pos: (x, 100, x + 50),
            rot: (x * 3, x * 5),
            stick_pos: (x, x + 1, x + 2),
            stick_rot: (x * 2, x * 4),
            head_rot: x + 10,
            body_rot: x + 20,
        })
    }

    #[test]
    fn decoder_discards_invalid_datagrams() {
        let mut codec = HQMMessageCodec;
        let mut datagram = BytesMut::from(&b"invalid"[..]);

        assert!(codec.decode(&mut datagram).is_err());
        assert!(datagram.is_empty());
    }

    #[test]
    fn bits_round_trip_at_every_offset_and_width() {
        for offset in 0..8 {
            for width in 1..=32 {
                for value in [0, mask(width), 0xA5C3_96E1 & mask(width)] {
                    let packet = write(|w| {
                        w.write_bits(offset, 0);
                        w.write_bits(width, value);
                        w.write_bits(5, 0b10110);
                    });
                    let mut reader = HQMMessageReader::new(&packet);
                    reader.read_bits(offset);

                    assert_eq!(
                        reader.read_bits(width),
                        value,
                        "offset {offset}, width {width}"
                    );
                    assert_eq!(reader.read_bits(5), 0b10110);
                }
            }
        }
    }

    #[test]
    fn write_bits_drops_bits_beyond_the_field_width() {
        let packet = write(|w| {
            w.write_bits(3, 0xFF);
            w.write_bits(5, 0);
        });

        assert_eq!(packet, [0b0000_0111]);
    }

    #[test]
    fn aligned_values_start_on_the_next_byte() {
        let packet = write(|w| {
            w.write_bits(3, 0b101);
            w.write_u32_aligned(0xDEAD_BEEF);
            w.write_bits(1, 1);
            w.write_f32_aligned(-1.25);
            w.write_byte_aligned(0x42);
            w.write_bytes_aligned_padded(6, b"abc");
            w.write_bytes_aligned_padded(2, b"xyz");
        });
        assert_eq!(packet.len(), 1 + 4 + 1 + 4 + 1 + 6 + 2);

        let mut reader = HQMMessageReader::new(&packet);
        assert_eq!(reader.read_bits(3), 0b101);
        assert_eq!(reader.read_u32_aligned(), 0xDEAD_BEEF);
        assert_eq!(reader.read_bits(1), 1);
        assert_eq!(reader.read_f32_aligned(), -1.25);
        assert_eq!(reader.read_byte_aligned(), 0x42);
        let mut padded = [0xFF; 6];
        reader.read_bytes_aligned(&mut padded);
        assert_eq!(&padded, b"abc\0\0\0");
        let mut truncated = [0; 2];
        reader.read_bytes_aligned(&mut truncated);
        assert_eq!(&truncated, b"xy");
    }

    #[test]
    fn reads_past_the_end_return_zeros() {
        let mut reader = HQMMessageReader::new(&[0xFF]);

        assert_eq!(reader.read_bits(4), 0xF);
        assert_eq!(reader.read_bits(8), 0xF);
        assert_eq!(reader.read_u32_aligned(), 0);
        assert_eq!(reader.read_u16_aligned(), 0);
    }

    #[test]
    fn positions_round_trip_with_the_smallest_delta_encoding() {
        let old = 5000;
        // (new value, expected size tag)
        let cases = [
            (old, 0),
            (old - 4, 0),
            (old + 3, 0),
            (old - 5, 1),
            (old + 4, 1),
            (old - 32, 1),
            (old + 31, 1),
            (old - 33, 2),
            (old + 32, 2),
            (old - 2048, 2),
            (old + 2047, 2),
            (old - 2049, 3),
            (old + 2048, 3),
            (0, 3),
            (mask(17), 3),
        ];

        for (value, tag) in cases {
            let packet = write(|w| w.write_pos(17, value, Some(old)));

            assert_eq!(
                HQMMessageReader::new(&packet).read_bits(2),
                tag,
                "value {value}"
            );
            assert_eq!(
                HQMMessageReader::new(&packet).read_pos(17, Some(old)),
                value,
                "value {value}"
            );
        }
    }

    #[test]
    fn positions_without_a_baseline_are_sent_in_full() {
        for value in [0, 1, 5000, mask(17)] {
            let packet = write(|w| w.write_pos(17, value, None));

            assert_eq!(HQMMessageReader::new(&packet).read_bits(2), 3);
            assert_eq!(HQMMessageReader::new(&packet).read_pos(17, None), value);
        }
    }

    #[test]
    fn directions_in_every_octant_survive_the_network_encoding() {
        for i in 0..12 {
            for j in 0..24 {
                let polar = PI * (i as f32 + 0.5) / 12.0;
                let azimuth = 2.0 * PI * j as f32 / 24.0;
                let direction = Vec3::new(
                    polar.sin() * azimuth.cos(),
                    polar.cos(),
                    polar.sin() * azimuth.sin(),
                );

                let encoded = convert_rot_column_to_network(31, direction);
                let decoded = convert_rot_column_from_network(31, encoded);

                assert!(
                    decoded.abs_diff_eq(direction, 1e-3),
                    "{direction} decoded as {decoded}"
                );
            }
        }
    }

    #[test]
    fn rotations_survive_the_network_encoding() {
        let rotations = [
            Rot3::IDENTITY,
            Rot3::from_rotation_y(PI),
            Rot3::from_rotation_y(1.0) * Rot3::from_rotation_x(-0.7),
            Rot3::from_rotation_z(2.5) * Rot3::from_rotation_y(-2.0) * Rot3::from_rotation_x(0.4),
        ];

        // Pucks and skaters use 31 bits, sticks 25
        for (bits, tolerance) in [(31, 1e-3), (25, 1e-2)] {
            for rot in rotations {
                let (v1, v2) = convert_matrix_to_network(bits, &rot);
                let decoded = convert_matrix_from_network(bits, v1, v2);

                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    assert!(
                        (decoded * axis).abs_diff_eq(rot * axis, tolerance),
                        "{bits} bits: {axis} became {} instead of {}",
                        decoded * axis,
                        rot * axis
                    );
                }
            }
        }
    }

    #[test]
    fn parses_server_info_requests() {
        let packet = client_packet(0, |w| {
            w.write_bits(8, 55);
            w.write_u32_aligned(1234);
        });

        let HQMClientToServerMessage::ServerInfo { version, ping } = parse(&packet) else {
            panic!("expected a server info request");
        };
        assert_eq!((version, ping), (55, 1234));
    }

    #[test]
    fn parses_join_requests() {
        let packet = client_packet(2, |w| {
            w.write_bits(8, 55);
            w.write_bytes_aligned_padded(32, b"Migo");
        });

        let HQMClientToServerMessage::Join {
            version,
            player_name,
        } = parse(&packet)
        else {
            panic!("expected a join request");
        };
        assert_eq!(version, 55);
        assert_eq!(player_name, "Migo");
    }

    #[test]
    fn join_names_without_a_terminator_use_all_32_bytes() {
        let packet = client_packet(2, |w| {
            w.write_bits(8, 55);
            w.write_bytes_aligned(&[b'x'; 32]);
        });

        let HQMClientToServerMessage::Join { player_name, .. } = parse(&packet) else {
            panic!("expected a join request");
        };
        assert_eq!(player_name, "x".repeat(32));
    }

    #[test]
    fn empty_join_names_become_noname() {
        let packet = client_packet(2, |w| w.write_bits(8, 55));

        let HQMClientToServerMessage::Join { player_name, .. } = parse(&packet) else {
            panic!("expected a join request");
        };
        assert_eq!(player_name, "Noname");
    }

    #[test]
    fn join_names_must_be_utf8() {
        let packet = client_packet(2, |w| {
            w.write_bits(8, 55);
            w.write_bytes_aligned_padded(32, &[0xFF, 0xFE]);
        });

        assert!(matches!(
            HQMMessageCodec.parse_message(&packet),
            Err(HQMDecodeError::StringDecoding(_))
        ));
    }

    #[test]
    fn parses_exit_requests() {
        assert!(matches!(
            parse(&client_packet(7, |_| {})),
            HQMClientToServerMessage::Exit
        ));
    }

    #[test]
    fn rejects_unknown_headers_and_commands() {
        let codec = HQMMessageCodec;

        assert!(matches!(
            codec.parse_message(b""),
            Err(HQMDecodeError::WrongHeader)
        ));
        assert!(matches!(
            codec.parse_message(b"Hack\x02"),
            Err(HQMDecodeError::WrongHeader)
        ));
        assert!(matches!(
            codec.parse_message(b"Hock\x03"),
            Err(HQMDecodeError::UnknownType)
        ));
    }

    #[test]
    fn parses_updates_from_each_client_version() {
        let versions = [
            (4, HQMClientVersion::Vanilla, None),
            (8, HQMClientVersion::Ping, Some(16)),
            (0x10, HQMClientVersion::PingRules, Some(16)),
        ];

        for (command, version, deltatime) in versions {
            let packet = update_packet(command, deltatime, no_chat);

            let HQMClientToServerMessage::Update {
                current_game_id,
                input,
                deltatime: parsed_deltatime,
                new_known_packet,
                known_msg_pos,
                chat,
                version: parsed_version,
            } = parse(&packet)
            else {
                panic!("expected an update");
            };
            assert_eq!(current_game_id, 7);
            assert_eq!(input.stick_angle, 0.5);
            assert_eq!(input.turn, -1.0);
            assert_eq!(input.fwbw, 0.25);
            assert_eq!(input.stick, Vec2::new(0.1, -0.2));
            assert_eq!(input.head_rot, 1.5);
            assert_eq!(input.body_rot, -1.5);
            assert_eq!(input.keys, 0b1010);
            assert_eq!(parsed_deltatime, deltatime);
            assert_eq!(new_known_packet, 321);
            assert_eq!(known_msg_pos, 17);
            assert_eq!(chat, None);
            assert!(parsed_version == version);
        }
    }

    #[test]
    fn parses_chat_in_updates() {
        let text = "hello /fs".as_bytes();
        let packet = update_packet(8, Some(16), with_chat(5, text.len(), text));

        let HQMClientToServerMessage::Update { chat, .. } = parse(&packet) else {
            panic!("expected an update");
        };
        assert_eq!(chat, Some((5, "hello /fs".to_owned())));
    }

    #[test]
    fn chat_must_be_utf8() {
        let packet = update_packet(4, None, with_chat(0, 2, &[0xC3, 0x28]));

        assert!(matches!(
            HQMMessageCodec.parse_message(&packet),
            Err(HQMDecodeError::StringDecoding(_))
        ));
    }

    #[test]
    fn truncated_updates_read_missing_bytes_as_zero() {
        let HQMClientToServerMessage::Update {
            current_game_id,
            deltatime,
            new_known_packet,
            chat,
            ..
        } = parse(b"Hock\x08")
        else {
            panic!("expected an update");
        };
        assert_eq!(current_game_id, 0);
        assert_eq!(deltatime, Some(0));
        assert_eq!(new_known_packet, 0);
        assert_eq!(chat, None);

        // A chat message claiming more bytes than the packet holds is padded instead of
        // reading out of bounds
        let packet = update_packet(4, None, with_chat(0, 10, b"hi"));
        let HQMClientToServerMessage::Update {
            chat: Some((_, text)),
            ..
        } = parse(&packet)
        else {
            panic!("expected an update with chat");
        };
        assert_eq!(text, "hi\0\0\0\0\0\0\0\0");
    }

    #[test]
    fn encodes_server_info_responses() {
        let mut buf = BytesMut::new();
        let message = HQMServerToClientMessage::ServerInfo {
            ping: 1234,
            player_count: 9,
            team_size: 5,
            server_name: "Migo",
        };
        assert!(HQMMessageCodec.encode(message, &mut buf).is_ok());

        let mut reader = HQMMessageReader::new(&buf);
        let mut header = [0; 4];
        reader.read_bytes_aligned(&mut header);
        assert_eq!(&header, GAME_HEADER);
        assert_eq!(reader.read_byte_aligned(), 1);
        assert_eq!(reader.read_bits(8), 55);
        assert_eq!(reader.read_u32_aligned(), 1234);
        assert_eq!(reader.read_bits(8), 9);
        assert_eq!(reader.read_bits(4), 4);
        assert_eq!(reader.read_bits(4), 5);
        let mut name = [0xFF; 32];
        reader.read_bytes_aligned(&mut name);
        assert_eq!(&name[..5], b"Migo\0");
        assert_eq!(buf.len(), 4 + 1 + 1 + 4 + 1 + 1 + 32);
    }

    #[test]
    fn chat_messages_are_sent_as_seven_bit_text() {
        let message = HQMMessage::Chat {
            player_index: Some(PlayerIndex(3)),
            message: Cow::Borrowed("Good game!"),
        };
        let packet = write(|w| write_message(w, &message));

        let mut reader = HQMMessageReader::new(&packet);
        assert_eq!(reader.read_bits(6), 2);
        assert_eq!(reader.read_bits(6), 3);
        let length = reader.read_bits(6);
        let text: String = (0..length)
            .map(|_| reader.read_bits(7) as u8 as char)
            .collect();
        assert_eq!(text, "Good game!");
    }

    #[test]
    fn server_chat_has_no_sender_and_is_cut_to_63_bytes() {
        let message = HQMMessage::Chat {
            player_index: None,
            message: Cow::Owned("x".repeat(100)),
        };
        let packet = write(|w| write_message(w, &message));

        let mut reader = HQMMessageReader::new(&packet);
        assert_eq!(reader.read_bits(6), 2);
        assert_eq!(reader.read_bits(6), 63);
        assert_eq!(reader.read_bits(6), 63);
    }

    #[test]
    fn goal_messages_name_the_team_scorer_and_assist() {
        let message = HQMMessage::Goal {
            team: Team::Blue,
            goal_player_index: Some(PlayerIndex(4)),
            assist_player_index: None,
        };
        let packet = write(|w| write_message(w, &message));

        let mut reader = HQMMessageReader::new(&packet);
        assert_eq!(reader.read_bits(6), 1);
        assert_eq!(reader.read_bits(2), Team::Blue.get_num());
        assert_eq!(reader.read_bits(6), 4);
        assert_eq!(reader.read_bits(6), 63);
    }

    #[test]
    fn objects_are_delta_encoded_against_the_clients_known_packet() {
        let mut history = PacketHistory::new();
        let known = history.push(objects(|o| {
            o[0] = skater(1000);
            o[1] = puck(2000);
            o[3] = puck(500);
            o[5] = skater(3000);
        }));
        // A small move, a big jump, a new object, an object changing type and one removed
        let current = objects(|o| {
            o[0] = skater(1002);
            o[1] = puck(9000);
            o[2] = puck(50);
            o[3] = skater(500);
        });
        let latest = history.push(current.clone());

        let delta = write(|w| write_objects(w, &history, Some(known)));
        let mut reader = HQMMessageReader::new(&delta);
        assert_eq!(reader.read_u32_aligned(), latest.to_wire());
        assert_eq!(reader.read_u32_aligned(), known.to_wire());
        assert_eq!(
            read_objects(&mut reader, history.objects_for(known)),
            current
        );

        let full = write(|w| write_objects(w, &history, None));
        assert!(delta.len() < full.len());
    }

    #[test]
    fn objects_are_sent_in_full_without_a_usable_baseline() {
        let mut history = PacketHistory::new();
        let current = objects(|o| {
            o[0] = skater(1000);
            o[1] = puck(2000);
        });
        let latest = history.push(current.clone());

        for known in [None, Some(latest), Some(PacketNumber(12345))] {
            let packet = write(|w| write_objects(w, &history, known));

            let mut reader = HQMMessageReader::new(&packet);
            assert_eq!(reader.read_u32_aligned(), latest.to_wire());
            assert_eq!(
                reader.read_u32_aligned(),
                known.map_or(u32::MAX, PacketNumber::to_wire)
            );
            assert_eq!(read_objects(&mut reader, None), current);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectPacket {
    None,
    Puck(PuckPacket),
    Skater(SkaterPacket),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkaterPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
    pub stick_pos: (u32, u32, u32),
    pub stick_rot: (u32, u32),
    pub head_rot: u32,
    pub body_rot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

            for b in message_bytes.iter().take(size).copied() {
                writer.write_bits(7, b as u32);
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
                Some((slot, team)) => (slot.index() as u32, team.get_num()),
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
    history: &PacketHistory,
    known_packet: Option<PacketNumber>,
) {
    let current_packets = history.current_objects();
    let old_packets = known_packet.and_then(|packet| history.baseline_objects_for(packet));

    writer.write_u32_aligned(history.latest_sequence().to_wire());
    writer.write_u32_aligned(known_packet.map_or(u32::MAX, PacketNumber::to_wire));

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
