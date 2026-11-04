//! Wire protocol for the Ocypus Sigma PRO L24 (TXW818-NV3052C-3.5inch-hor).
//!
//! Reverse-engineered from the official Electron app (`_baseClass/device.js`)
//! and verified live against the device.
//!
//! Command frame (host -> device):
//!   [0x55, 0xAA, len_lo, len_hi, key, payload..., sum_lo, sum_hi]
//!   len = payload_len + 7, sum = (sum of all preceding bytes) & 0xFFFF
//!
//! Response frame (device -> host): identical layout; byte 4 echoes the
//! command key; payload is UTF-8 JSON or a 1-byte status code.
//!
//! Live-image stream: after `key 0x11` (live mode), each frame is
//!   [u32 LE jpeg_len][jpeg bytes][u16 LE sum]   (firmware > 2.8)
//! written raw to the port (no 0x55 0xAA wrapper). JPEG must be <= ~80KB.

pub const MAGIC: [u8; 2] = [0x55, 0xAA];
#[allow(dead_code)]
pub const KEY_RESTART: u8 = 0x01;
pub const KEY_GET_INFO: u8 = 0x06;
#[allow(dead_code)]
pub const KEY_FW_UPDATE: u8 = 0x0C;
pub const KEY_LIVE: u8 = 0x11;
#[allow(dead_code)]
pub const KEY_MOTION_BEFORE_OFF: u8 = 0x14;
pub const KEY_MOTION_TIMEOUT: u8 = 0x15;
#[allow(dead_code)]
pub const KEY_SET_REGION: u8 = 0x20;
#[allow(dead_code)]
pub const KEY_CLOSE: u8 = 0x21;
#[allow(dead_code)]
pub const KEY_SET_SERIAL: u8 = 0x23;
#[allow(dead_code)]
pub const KEY_MOTOR: u8 = 0x25;
pub const KEY_RT_PLAY_TIMEOUT: u8 = 0x26;

/// Max JPEG payload the firmware accepts per frame (KB), version > 2.8.
pub const JPEG_MAX_KB: usize = 80;
/// Serial chunk size used by the official uploader (not required for JPEG).
#[allow(dead_code)]
pub const FILE_CHUNK: usize = 20 * 1024;

pub fn cmd_frame(key: u8, value: &[u8]) -> Vec<u8> {
    let len = (value.len() + 7) as u16;
    let mut f = Vec::with_capacity(len as usize);
    f.extend_from_slice(&MAGIC);
    f.push((len & 0xFF) as u8);
    f.push((len >> 8) as u8);
    f.push(key);
    f.extend_from_slice(value);
    let sum = f.iter().fold(0u32, |a, &b| a + b as u32) & 0xFFFF;
    f.push((sum & 0xFF) as u8);
    f.push((sum >> 8) as u8);
    f
}

/// Live-stream JPEG frame: u32 LE length + jpeg + u16 LE checksum
/// (checksum covers length+payload). Firmware version > 2.8 only.
pub fn jpeg_frame(jpeg: &[u8]) -> Vec<u8> {
    let n = jpeg.len() as u32;
    let mut f = Vec::with_capacity(n as usize + 6);
    f.extend_from_slice(&n.to_le_bytes());
    f.extend_from_slice(jpeg);
    let sum = f.iter().fold(0u32, |a, &b| a + b as u32) & 0xFFFF;
    f.push((sum & 0xFF) as u8);
    f.push((sum >> 8) as u8);
    f
}

/// "Clear / stop" sequence sent on shutdown (two JPEG EOI markers).
pub const CLEAR_SEQ: [u8; 4] = [0xFF, 0xD9, 0xFF, 0xD9];

pub struct FrameParser {
    buf: Vec<u8>,
}

/// Parsed device response.
pub enum Response {
    /// Command echo + raw payload bytes (5..len-2)
    Frame { key: u8, payload: Vec<u8> },
}

impl FrameParser {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(8192) }
    }

    pub fn push(&mut self, data: &[u8], out: &mut Vec<Response>) {
        self.buf.extend_from_slice(data);
        loop {
            // resync to magic
            let pos = self
                .buf
                .windows(2)
                .position(|w| w == MAGIC);
            match pos {
                Some(0) => {}
                Some(p) => {
                    self.buf.drain(..p);
                }
                None => {
                    // keep last byte (could be 0x55)
                    let keep = self.buf.last() == Some(&0x55);
                    let n = self.buf.len() - keep as usize;
                    self.buf.drain(..n);
                    return;
                }
            }
            if self.buf.len() < 4 {
                return;
            }
            let len = self.buf[2] as usize | ((self.buf[3] as usize) << 8);
            if len < 7 || len > 0xFFFF {
                self.buf.drain(..2);
                continue;
            }
            if self.buf.len() < len {
                return;
            }
            let frame: Vec<u8> = self.buf.drain(..len).collect();
            out.push(Response::Frame {
                key: frame[4],
                payload: frame[5..frame.len() - 2].to_vec(),
            });
        }
    }
}
