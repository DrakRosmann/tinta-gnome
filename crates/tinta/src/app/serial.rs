//! The serial monitor, in a panel of its own: what the board writes, with the time it arrived,
//! lines sent to it, and a plot of the numbers in what it writes. It lets go of the port while
//! the board is being written to, and connects again after.

use super::{App, Message, PanelView};
use crate::bus;
use crate::fx::Task;
use crate::i18n::tr;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;
use tinta_core::embedded::{self, prefs};
use tinta_core::embedded::serial::{self, Decoder, LineEnding, Plot, PortEvent};

pub const SERIAL_INPUT: &str = "serial-input";
const MAX_LINES: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// From the board.
    Received,
    /// Typed and sent to it.
    Sent,
    /// About the connection.
    Info,
    Error,
}

#[derive(Debug, Clone)]
pub struct SerialLine {
    pub time: String,
    pub text: String,
    pub kind: LineKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SerialState {
    #[default]
    Off,
    /// Connecting, or the board went away and it waits for it.
    Waiting,
    Open(String),
}

#[derive(Default)]
pub struct SerialPanel {
    pub state: SerialState,
    pub generation: u64,
    /// The bridge's process group.
    pid: Option<u32>,
    writer: Option<Sender<Vec<u8>>>,
    pub lines: Vec<SerialLine>,
    /// Lines ever added: tells the view what's new, also after old ones are dropped.
    pub total: u64,
    /// Bumped when the lines are cleared.
    pub epoch: u64,
    decoder: Decoder,
    /// The line still coming in.
    pub partial: String,
    pub input: String,
    pub ending: LineEnding,
    pub timestamps: bool,
    pub show_plot: bool,
    pub plot: Plot,
    /// What it's connected to, or tries to: the port (None: the first plugged in) and the speed.
    pub target: (Option<String>, u32),
    /// An upload under way: its task's generation, and whether to connect when it's done.
    pub after_upload: Option<(u64, bool)>,
}

impl SerialPanel {
    pub fn is_on(&self) -> bool {
        self.pid.is_some()
    }

    fn push(&mut self, kind: LineKind, text: String) {
        self.lines.push(SerialLine { time: serial::clock(), text, kind });
        self.total += 1;
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
    }
}

#[derive(Debug, Clone)]
pub enum SerialMsg {
    Data(u64, Vec<u8>),
    Event(u64, String),
    Ended(u64),
    Input(String),
    Send,
    /// Connect, or disconnect.
    Toggle,
    Clear,
    Timestamps(bool),
    Plot(bool),
    Ending(LineEnding),
    Baud(u32),
}

/// Bytes from the bridge, a batch every few milliseconds: a chatty board doesn't flood the app.
fn pump_bytes(generation: u64, from: impl Read + Send + 'static) {
    let (tx, rx) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut from = from;
        let mut buf = [0u8; 4096];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut batch = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(b) => {
                    batch.extend_from_slice(&b);
                    if batch.len() < 64 * 1024 {
                        continue;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        bus::send(Message::Serial(SerialMsg::Data(generation, batch)));
                    }
                    return;
                }
            }
            if !batch.is_empty() {
                bus::send(Message::Serial(SerialMsg::Data(generation, std::mem::take(&mut batch))));
            }
        }
    });
}

impl App {
    /// The port and speed: the firmware project's, or what was chosen with no project open.
    pub fn serial_target(&self) -> (Option<String>, u32) {
        match &self.embedded.current {
            Some(p) => (p.port.clone(), p.baud),
            None => {
                let p = prefs::load(&self.prefs_root());
                (p.port, p.baud.unwrap_or(embedded::DEFAULT_BAUD))
            }
        }
    }

    /// Connects, or connects again with the port and speed chosen now.
    pub fn serial_connect(&mut self) -> Task {
        self.serial_stop();
        let (port, baud) = self.serial_target();
        self.serial.target = (port.clone(), baud);
        self.serial.generation += 1;
        let g = self.serial.generation;
        let argv = serial::bridge_argv(port.as_deref(), baud);
        let child = tinta_core::host::command(&argv[0], &argv[1..], None, &[])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return self.toast(format!("sh: {e}")),
        };
        if let Some(out) = child.stdout.take() {
            pump_bytes(g, out);
        }
        if let Some(err) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    bus::send(Message::Serial(SerialMsg::Event(g, line)));
                }
            });
        }
        let (tx, rx) = channel::<Vec<u8>>();
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                while let Ok(b) = rx.recv() {
                    if stdin.write_all(&b).and_then(|()| stdin.flush()).is_err() {
                        break;
                    }
                }
            });
        }
        self.serial.pid = Some(child.id());
        std::thread::spawn(move || {
            let _ = child.wait();
            bus::send(Message::Serial(SerialMsg::Ended(g)));
        });
        self.serial.writer = Some(tx);
        self.serial.state = SerialState::Waiting;
        self.serial.decoder = Decoder::default();
        self.serial.partial.clear();
        let name = port.unwrap_or_else(|| tr("the first port plugged in").to_string());
        let text = if self.pt() { format!("Conectando a {name}, {baud} baud…") } else { format!("Connecting to {name}, {baud} baud…") };
        self.serial.push(LineKind::Info, text);
        Task::none()
    }

    /// Ends the bridge, quietly. Whether it was connected.
    fn serial_stop(&mut self) -> bool {
        self.serial.writer = None;
        let Some(pid) = self.serial.pid.take() else { return false };
        self.serial.generation += 1;
        self.serial.state = SerialState::Off;
        let _ = Command::new("kill").arg("--").arg(format!("-{pid}")).stderr(Stdio::null()).status();
        true
    }

    /// Lets go of the port, saying so. Whether it was connected.
    pub fn serial_disconnect(&mut self) -> bool {
        let was = self.serial_stop();
        if was {
            self.serial.push(LineKind::Info, tr("Disconnected").to_string());
        }
        was
    }

    /// The monitor in front, connected.
    pub fn serial_show(&mut self, plot: bool) -> Task {
        self.panel = Some(PanelView::Serial);
        if plot {
            self.serial.show_plot = true;
        }
        let t = if self.serial.is_on() { Task::none() } else { self.serial_connect() };
        Task::batch([t, crate::fx::focus(SERIAL_INPUT)])
    }

    /// After a new port or speed is chosen: a connected monitor follows.
    pub fn serial_follow_choice(&mut self) -> Task {
        if self.serial.is_on() { self.serial_connect() } else { Task::none() }
    }

    /// An upload starts: the port is freed, and taken back once it's done.
    pub fn serial_upload_started(&mut self, generation: u64, then_watch: bool) {
        let was = self.serial_disconnect();
        self.serial.after_upload = Some((generation, was || then_watch));
    }

    pub fn serial_upload_finished(&mut self, generation: u64, success: bool) -> Task {
        let Some((g, reconnect)) = self.serial.after_upload else { return Task::none() };
        if g != generation {
            return Task::none();
        }
        self.serial.after_upload = None;
        if !reconnect {
            return Task::none();
        }
        if success {
            self.serial.push(LineKind::Info, tr("Uploaded to the board").to_string());
            return self.serial_show(false);
        }
        // The errors stay in front; the monitor just goes back to listening.
        self.serial_connect()
    }

    pub fn on_serial(&mut self, m: SerialMsg) -> Task {
        let s = &mut self.serial;
        match m {
            SerialMsg::Data(g, bytes) => {
                if g != s.generation {
                    return Task::none();
                }
                for line in s.decoder.feed(&bytes) {
                    s.plot.push_line(&line);
                    s.push(LineKind::Received, line);
                }
                s.partial = s.decoder.partial();
            }
            SerialMsg::Event(g, line) => {
                if g != s.generation {
                    return Task::none();
                }
                let pt = crate::i18n::is_portuguese();
                match serial::parse_event(&line) {
                    Some(PortEvent::Waiting) => s.state = SerialState::Waiting,
                    Some(PortEvent::Open(p)) => {
                        let text = if pt { format!("Conectado a {p}, {} baud", s.target.1) } else { format!("Connected to {p}, {} baud", s.target.1) };
                        s.state = SerialState::Open(p);
                        s.push(LineKind::Info, text);
                    }
                    Some(PortEvent::Lost) => {
                        s.state = SerialState::Waiting;
                        s.push(LineKind::Info, tr("The board went away: waiting for it to come back…").to_string());
                    }
                    Some(PortEvent::NotFound) => {
                        s.push(LineKind::Error, tr("No serial port found: plug the board in").to_string());
                    }
                    None if !line.trim().is_empty() => s.push(LineKind::Error, line),
                    None => {}
                }
            }
            SerialMsg::Ended(g) => {
                if g == s.generation {
                    s.pid = None;
                    s.writer = None;
                    s.state = SerialState::Off;
                }
            }
            SerialMsg::Input(t) => s.input = t,
            SerialMsg::Send => {
                let Some(w) = &s.writer else { return self.toast(tr("Connect to the board first")) };
                let mut bytes = s.input.clone().into_bytes();
                bytes.extend_from_slice(s.ending.bytes());
                if w.send(bytes).is_err() {
                    return self.toast(tr("Connect to the board first"));
                }
                let text = std::mem::take(&mut s.input);
                s.push(LineKind::Sent, text);
            }
            SerialMsg::Toggle => {
                return if s.is_on() {
                    self.serial_disconnect();
                    Task::none()
                } else {
                    self.serial_connect()
                };
            }
            SerialMsg::Clear => {
                s.lines.clear();
                s.partial.clear();
                s.epoch += 1;
                s.plot.clear();
            }
            SerialMsg::Timestamps(on) => s.timestamps = on,
            SerialMsg::Plot(on) => s.show_plot = on,
            SerialMsg::Ending(e) => s.ending = e,
            SerialMsg::Baud(b) => return self.set_serial_baud(b),
        }
        Task::none()
    }

    /// When everything's closing: the bridge mustn't outlive the editor, or it holds the port.
    pub fn serial_shutdown(&mut self) {
        self.serial_stop();
    }
}
