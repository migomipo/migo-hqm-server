use std::cmp::min;
use nalgebra::{Vector3, U1, U3, Matrix3};
use nalgebra::storage::Storage;

pub fn convert_matrix(n: u8, v: &Matrix3<f32>) -> (u32, u32) {
    let r1 = convert_rot_row(n, &v.column(1));
    let r2 = convert_rot_row(n, &v.column(2));
    (r1, r2)
}

fn convert_rot_row<S: Storage<f32, U3, U1>>(n: u8, v: &nalgebra::Matrix<f32, U3, U1, S>) -> u32 {

    let uxp = Vector3::x();
    let uxn = -uxp;
    let uyp = Vector3::y();
    let uyn = -uyp;
    let uzp = Vector3::z();
    let uzn = -uzp;

    let a = [
        [&uyp, &uxp, &uzp],
        [&uyp, &uzp, &uxn],
        [&uyp, &uzn, &uxp],
        [&uyp, &uxn, &uzn],
        [&uzp, &uxp, &uyn],
        [&uxn, &uzp, &uyn],
        [&uxp, &uzn, &uyn],
        [&uzn, &uxn, &uyn]
    ];

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
    let mut temp1 = a[res as usize][0].clone();
    let mut temp2 = a[res as usize][1].clone();
    let mut temp3 = a[res as usize][2].clone();
    for i in (3..n).step_by(2) {
        let temp4 = (temp1 + temp2).normalize ();
        let temp5 = (temp2 + temp3).normalize ();
        let temp6 = (temp1 + temp3).normalize ();

        let a1 = (temp4-temp6).cross(&(v-temp6));
        if a1.dot(&v) < 0.0 {
            let a2 = (temp5-temp4).cross(&(v-temp4));
            if a2.dot(&v) < 0.0 {
                let a3 = (temp6-temp5).cross(&(v-temp5));
                if a3.dot (&v) < 0.0 {
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


pub enum HQMObjectPacket {
    None,
    Puck(HQMPuckPacket),
    Skater(HQMSkaterPacket)
}

pub struct HQMSkaterPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
    pub stick_pos: (u32, u32, u32),
    pub stick_rot: (u32, u32),
    pub head_rot: u32,
    pub body_rot: u32,
}

pub struct HQMPuckPacket {
    pub pos: (u32, u32, u32),
    pub rot: (u32, u32),
}

pub struct HQMServerWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
    bit_pos: u8,
}

impl<'a> HQMServerWriter<'a> {
    pub fn get_slice(&self) -> &[u8] {
        let size = self.get_bytes_written();
        return &self.buf[0..size];
    }

    pub fn get_bytes_written(&self) -> usize {
        return if self.bit_pos > 0 { self.pos + 1 } else { self.pos };
    }

    pub fn write_byte_aligned(&mut self, v: u8) {
        self.align();
        self.buf[self.pos] = v;
        self.pos += 1;
    }

    pub fn write_bytes_aligned(&mut self, v: &[u8]) {
        self.align();
        for b in v {
            self.buf[self.pos] = *b;
            self.pos += 1;
        }
    }

    pub fn write_bytes_aligned_padded(&mut self, n: usize, v: &[u8]) {
        self.align();
        let m = min(n, v.len());
        self.write_bytes_aligned(&v[0..m]);
        for _ in 0..(n - m) {
            self.buf[self.pos] = 0;
            self.pos += 1;
        }
    }

    pub fn write_u32_aligned(&mut self, v: u32) {
        self.align();
        self.buf[self.pos] = (v & 0xff) as u8;
        self.buf[self.pos + 1] = ((v >> 8) & 0xff) as u8;
        self.buf[self.pos + 2] = ((v >> 16) & 0xff) as u8;
        self.buf[self.pos + 3] = ((v >> 24) & 0xff) as u8;
        self.pos += 4;
    }

    pub fn write_f32_aligned(&mut self, v: f32) {
        self.write_u32_aligned(f32::to_bits(v));
    }

    pub fn write_pos(&mut self, n: u8, v: u32) {
        self.write_bits(2, 3);
        self.write_bits(n, v);
    }

    pub fn write_bits(&mut self, n: u8, v: u32) {
        let to_write = if n < 32 { !(u32::MAX << n) & v } else { v };
        let mut bits_remaining = n;
        let mut p = 0;
        while bits_remaining > 0 {
            let bits_possible_to_write = 8 - self.bit_pos;
            let bits = min(bits_remaining, bits_possible_to_write);
            let mask = !(!0 << bits);
            let a = (((to_write >> p) & mask) << self.bit_pos) as u8;

            self.buf[self.pos] |= a;

            if bits_remaining >= bits_possible_to_write {
                bits_remaining -= bits_possible_to_write;
                self.pos += 1;
                self.bit_pos = 0;
                p += bits;
            } else {
                self.bit_pos += bits;
                bits_remaining = 0;
            }
        }
    }

    fn align(&mut self) {
        if self.bit_pos > 0 {
            self.bit_pos = 0;
            self.pos += 1;
        }
    }

    pub fn new(buf: &'a mut [u8]) -> Self {
        HQMServerWriter { buf, pos: 0, bit_pos: 0 }
    }
}

pub struct HQMClientParser<'a> {
    buf: &'a [u8],
    pos: usize,
    bit_pos: u8,
}

impl<'a> HQMClientParser<'a> {
    pub fn read_byte_aligned(&mut self) -> u8 {
        self.align();
        let res = self.buf[self.pos];
        self.pos = self.pos + 1;
        return res;
    }

    pub fn read_bytes_aligned(&mut self, n: usize) -> Vec<u8> {
        self.align();

        let mut res = Vec::with_capacity(n);
        for i in self.pos..(self.pos + n) {
            res.push(self.buf[i])
        }
        self.pos = self.pos + n;
        return res;
    }

    pub fn read_u16_aligned(&mut self) -> u16 {
        self.align();
        let b1 = self.buf[self.pos] as u16;
        let b2 = self.buf[self.pos + 1] as u16;
        self.pos = self.pos + 2;
        return b1 | b2 << 8;
    }

    pub fn read_u32_aligned(&mut self) -> u32 {
        self.align();
        let b1 = self.buf[self.pos] as u32;
        let b2 = self.buf[self.pos + 1] as u32;
        let b3 = self.buf[self.pos + 2] as u32;
        let b4 = self.buf[self.pos + 3] as u32;
        self.pos = self.pos + 4;
        return b1 | b2 << 8 | b3 << 16 | b4 << 24;
    }

    pub fn read_f32_aligned(&mut self) -> f32 {
        let i = self.read_u32_aligned();
        return f32::from_bits(i);
    }

    pub fn read_bits(&mut self, b: u8) -> u32 {
        let mut bits_remaining = b;
        let mut res = 0u32;
        let mut p = 0;
        while bits_remaining > 0 {
            let bits_possible_to_write = 8 - self.bit_pos;
            let bits = min(bits_remaining, bits_possible_to_write);
            let mask = !(!0u32 << bits);
            let a = (self.buf[self.pos] as u32 >> self.bit_pos) & mask;

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

    fn align(&mut self) {
        if self.bit_pos > 0 {
            self.bit_pos = 0;
            self.pos += 1;
        }
    }
    pub fn new(buf: &'a [u8]) -> Self {
        HQMClientParser { buf, pos: 0, bit_pos: 0 }
    }
}
