//! Display engine — owns the serial port on a dedicated writer thread.
//! Producer hands over pre-encoded JPEG frames via a latest-wins slot;
//! the thread also injects the 1.5s `0x11` keepalive, drains device
//! responses, and auto-reconnects on write failure.

use crate::protocol::{
    cmd_frame, jpeg_frame, FrameParser, Response, KEY_GET_INFO, KEY_LIVE,
    KEY_MOTION_TIMEOUT, KEY_RT_PLAY_TIMEOUT,
};
use serialport::{SerialPort, SerialPortType};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const VID: u16 = 0x33C3;
pub const PID: u16 = 0x7789;
pub const BAUD: u32 = 2_000_000;
const KEEPALIVE: Duration = Duration::from_millis(1500);
const RECONNECT: Duration = Duration::from_millis(1500);

#[derive(Default, Clone)]
pub struct EngineStats {
    pub connected: bool,
    pub port: String,
    pub frames_sent: u64,
    pub last_jpeg_bytes: usize,
    pub last_err: String,
}

#[derive(Clone)]
pub struct EngineHandle {
    slot: Arc<(Mutex<Option<Vec<u8>>>, Condvar)>,
    pub stats: Arc<Mutex<EngineStats>>,
    stop: Arc<AtomicBool>,
}

impl EngineHandle {
    /// Queue a pre-encoded JPEG frame (latest wins; slow frames are dropped).
    pub fn push_frame(&self, jpeg: Vec<u8>) {
        let (m, cv) = &*self.slot;
        *m.lock().unwrap() = Some(jpeg);
        cv.notify_one();
    }

    pub fn stats(&self) -> EngineStats {
        self.stats.lock().unwrap().clone()
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.slot.1.notify_one();
    }
}

pub fn find_port() -> Option<String> {
    let ports = serialport::available_ports().ok()?;
    for p in &ports {
        if let SerialPortType::UsbPort(u) = &p.port_type {
            if u.vid == VID && u.pid == PID {
                return Some(p.port_name.clone());
            }
        }
    }
    None
}

fn open_port(path: &str) -> std::io::Result<Box<dyn SerialPort>> {
    let mut port = serialport::new(path, BAUD)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(50))
        .open()?;
    let _ = port.write_data_terminal_ready(true);
    Ok(port)
}

fn reader_thread(mut port: Box<dyn SerialPort>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut parser = FrameParser::new();
        let mut responses = Vec::new();
        let mut buf = [0u8; 8192];
        while !stop.load(Ordering::Relaxed) {
            match port.read(&mut buf) {
                Ok(n) if n > 0 => {
                    parser.push(&buf[..n], &mut responses);
                    for r in responses.drain(..) {
                        let Response::Frame { key, payload } = r;
                        match std::str::from_utf8(&payload) {
                            Ok(t) if t.starts_with('{') => {
                                eprintln!("[dev <-] key=0x{key:02X} {t}")
                            }
                            _ => {
                                if payload != [0u8] {
                                    eprintln!("[dev <-] key=0x{key:02X} {payload:02X?}")
                                }
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    });
}

/// Start the engine on a background thread. Reconnects forever on errors.
pub fn start(port_name: Option<String>, stop: Arc<AtomicBool>) -> EngineHandle {
    let slot = Arc::new((Mutex::new(Option::<Vec<u8>>::None), Condvar::new()));
    let stats = Arc::new(Mutex::new(EngineStats::default()));
    let handle = EngineHandle {
        slot: slot.clone(),
        stats: stats.clone(),
        stop: stop.clone(),
    };
    std::thread::spawn(move || engine_loop(port_name, slot, stats, stop));
    handle
}

fn engine_loop(
    mut port_name: Option<String>,
    slot: Arc<(Mutex<Option<Vec<u8>>>, Condvar)>,
    stats: Arc<Mutex<EngineStats>>,
    stop: Arc<AtomicBool>,
) {
    let mut port: Option<Box<dyn SerialPort>> = None;
    let mut last_keepalive = Instant::now() - KEEPALIVE;

    while !stop.load(Ordering::Relaxed) {
        // (re)connect phase
        if port.is_none() {
            let name = port_name.clone().or_else(find_port);
            match name.as_deref().map(open_port) {
                Some(Ok(p)) => {
                    let mut p = p;
                    if let Ok(rx) = p.try_clone() {
                        reader_thread(rx, stop.clone());
                    }
                    handshake(&mut *p);
                    port = Some(p);
                    port_name = name;
                    {
                        let mut st = stats.lock().unwrap();
                        st.connected = true;
                        st.port = port_name.clone().unwrap_or_default();
                        st.last_err.clear();
                        eprintln!("[engine] connected {}", st.port);
                    }
                    let _ = port.as_mut().unwrap().write_all(&cmd_frame(KEY_LIVE, &[]));
                    last_keepalive = Instant::now();
                }
                _ => {
                    stats.lock().unwrap().connected = false;
                    stats.lock().unwrap().last_err = "device not found".into();
                    std::thread::sleep(RECONNECT);
                    continue;
                }
            }
        }

        // take newest frame or wait for keepalive deadline
        let (m, cv) = &*slot;
        let frame = {
            let mut g = m.lock().unwrap();
            if g.is_none() {
                let remain = KEEPALIVE.saturating_sub(last_keepalive.elapsed());
                let (g2, _) = cv.wait_timeout(g, remain).unwrap();
                g = g2;
            }
            g.take()
        };

        let p = port.as_mut().unwrap();
        let mut failed = false;
        if let Some(jpeg) = frame {
            let n = jpeg.len();
            let frame = jpeg_frame(&jpeg);
            if p.write_all(&frame).and_then(|_| p.flush()).is_err() {
                failed = true;
            } else {
                let mut st = stats.lock().unwrap();
                st.frames_sent += 1;
                st.last_jpeg_bytes = n;
            }
        } else if last_keepalive.elapsed() >= KEEPALIVE {
            if p.write_all(&cmd_frame(KEY_LIVE, &[]))
                .and_then(|_| p.flush())
                .is_err()
            {
                failed = true;
            } else {
                last_keepalive = Instant::now();
            }
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }

        if failed {
            eprintln!("[engine] write failed — reconnecting");
            stats.lock().unwrap().connected = false;
            stats.lock().unwrap().last_err = "write failed".into();
            port = None;
            std::thread::sleep(RECONNECT);
        }
    }

    // graceful: clear screen
    if let Some(p) = port.as_mut() {
        let _ = p.write_all(&crate::protocol::CLEAR_SEQ);
        let _ = p.flush();
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn handshake(port: &mut dyn SerialPort) {
    let _ = port.write_all(&cmd_frame(KEY_GET_INFO, &[]));
    let _ = port.flush();
    std::thread::sleep(Duration::from_millis(300));
    let _ = port.write_all(&cmd_frame(KEY_MOTION_TIMEOUT, &[1, 250]));
    let _ = port.write_all(&cmd_frame(KEY_RT_PLAY_TIMEOUT, &[5]));
    let _ = port.flush();
}
