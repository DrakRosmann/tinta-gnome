//! The serial monitor's insides: a bridge to the port, run on the computer itself (also from a
//! Flatpak), bytes split into lines, what's sent at the end of a line, and the numbers in each
//! line for the plotter.

use std::collections::VecDeque;

/// Copies the port to its standard output and its standard input to the port, raw. Says how the
/// port is doing on standard error, a line each: `@open PORT`, `@waiting`, `@lost`, `@none`.
/// Waits for the port (a board that just got its firmware may still be coming back), and again
/// whenever it goes away. Arguments: the port (empty: the first one to show up) and the speed.
pub const BRIDGE_SCRIPT: &str = r#"want=$1 b=$2
pick() {
  if [ -n "$want" ]; then [ -e "$want" ] && p=$want && return 0; return 1; fi
  for c in /dev/serial/by-id/* /dev/ttyACM* /dev/ttyUSB*; do
    [ -e "$c" ] && { p=$c; return 0; }
  done
  return 1
}
wait_port() {
  n=0
  until pick; do
    n=$((n + 1))
    [ "$1" -gt 0 ] && [ "$n" -ge "$1" ] && return 1
    sleep 0.1
  done
}
echo @waiting >&2
wait_port 100 || { echo @none >&2; exit 2; }
exec 3<&0
# Writes go to the port as it is now: after a reset it may be back under the same name.
( until cat <&3 > "$p" 2>/dev/null; do sleep 0.3; pick; done ) &
trap 'kill $! 2>/dev/null; exit 0' INT TERM HUP
while :; do
  if stty -F "$p" "$b" raw -echo -hupcl clocal -crtscts 2>/dev/null; then
    echo "@open $p" >&2
    cat "$p"
  fi
  echo @lost >&2
  sleep 0.5
  wait_port 0
done"#;

/// The command line that runs the bridge.
pub fn bridge_argv(port: Option<&str>, baud: u32) -> Vec<String> {
    vec!["sh".into(), "-c".into(), BRIDGE_SCRIPT.into(), "tinta-serial".into(), port.unwrap_or("").into(), baud.to_string()]
}

/// What the bridge says about the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortEvent {
    Waiting,
    Open(String),
    Lost,
    /// No port showed up in time.
    NotFound,
}

pub fn parse_event(line: &str) -> Option<PortEvent> {
    let line = line.trim();
    Some(match line {
        "@waiting" => PortEvent::Waiting,
        "@lost" => PortEvent::Lost,
        "@none" => PortEvent::NotFound,
        _ => PortEvent::Open(line.strip_prefix("@open ")?.to_string()),
    })
}

/// What goes after each line sent, as in the Arduino IDE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    None,
    #[default]
    Lf,
    Cr,
    CrLf,
}

impl LineEnding {
    pub const ALL: [LineEnding; 4] = [LineEnding::None, LineEnding::Lf, LineEnding::Cr, LineEnding::CrLf];

    pub fn bytes(self) -> &'static [u8] {
        match self {
            LineEnding::None => b"",
            LineEnding::Lf => b"\n",
            LineEnding::Cr => b"\r",
            LineEnding::CrLf => b"\r\n",
        }
    }

    pub fn label(self, pt: bool) -> &'static str {
        match (self, pt) {
            (LineEnding::None, true) => "Sem fim de linha",
            (LineEnding::None, false) => "No line ending",
            (LineEnding::Lf, true) => "Nova linha (LF)",
            (LineEnding::Lf, false) => "New line (LF)",
            (LineEnding::Cr, true) => "Retorno (CR)",
            (LineEnding::Cr, false) => "Carriage return (CR)",
            (LineEnding::CrLf, _) => "CR + LF",
        }
    }
}

/// Bytes from the port, into lines. A line ends at `\n`, `\r\n` or a lone `\r`; what comes after
/// the last end waits for more. Text that isn't UTF-8 still shows, with replacement marks.
#[derive(Debug, Default, Clone)]
pub struct Decoder {
    pending: Vec<u8>,
    /// The last chunk ended in `\r`: a `\n` starting the next one belongs to it.
    after_cr: bool,
}

impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            if self.after_cr {
                self.after_cr = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\n' => out.push(self.take()),
                b'\r' => {
                    out.push(self.take());
                    self.after_cr = true;
                }
                _ => self.pending.push(b),
            }
        }
        out
    }

    fn take(&mut self) -> String {
        let line = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        // Control characters other than tabs would only draw boxes.
        line.chars().filter(|c| !c.is_control() || *c == '\t').collect()
    }

    /// The line still coming in.
    pub fn partial(&self) -> String {
        String::from_utf8_lossy(&self.pending).chars().filter(|c| !c.is_control() || *c == '\t').collect()
    }
}

/// The numbers in a line, as the Arduino plotter reads them: separated by spaces, tabs or commas,
/// each one optionally named (`temp:21.5`). Unnamed ones are named by their place.
pub fn parse_values(line: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut place = 0;
    for field in line.split([' ', '\t', ',', ';']).map(str::trim).filter(|f| !f.is_empty()) {
        place += 1;
        let (name, value) = match field.rsplit_once([':', '=']) {
            Some((n, v)) => (n.trim().to_string(), v.trim()),
            None => (place.to_string(), field),
        };
        match value.parse::<f64>() {
            Ok(v) if v.is_finite() => out.push((name, v)),
            // A line of words isn't data.
            _ if out.is_empty() && field.chars().any(|c| c.is_alphabetic()) && !field.contains([':', '=']) => return Vec::new(),
            _ => {}
        }
    }
    out
}

/// The plotter's series: the last values of each name.
#[derive(Debug, Clone)]
pub struct Plot {
    pub series: Vec<(String, VecDeque<f64>)>,
    /// How many points each series keeps.
    pub width: usize,
    /// Bumped on every change, for whoever draws it.
    pub version: u64,
}

impl Default for Plot {
    fn default() -> Self {
        Self { series: Vec::new(), width: 500, version: 0 }
    }
}

impl Plot {
    /// A line from the board: its numbers go on the plot. Series that weren't in the line repeat
    /// their last value, so they all move along together.
    pub fn push_line(&mut self, line: &str) -> bool {
        let values = parse_values(line);
        if values.is_empty() {
            return false;
        }
        let len = self.series.first().map_or(0, |(_, v)| v.len());
        for (name, v) in &values {
            if !self.series.iter().any(|(n, _)| n == name) && self.series.len() < 12 {
                // A new series starts where the others are, with its first value.
                self.series.push((name.clone(), std::iter::repeat_n(*v, len.saturating_sub(1)).collect()));
            }
        }
        for (name, points) in &mut self.series {
            let v = values.iter().find(|(n, _)| n == name).map(|(_, v)| *v).or_else(|| points.back().copied()).unwrap_or(0.0);
            points.push_back(v);
            while points.len() > self.width {
                points.pop_front();
            }
        }
        self.version += 1;
        true
    }

    pub fn clear(&mut self) {
        self.series.clear();
        self.version += 1;
    }

    /// The lowest and highest value shown, a little apart when they're the same.
    pub fn range(&self) -> Option<(f64, f64)> {
        let mut it = self.series.iter().flat_map(|(_, v)| v.iter().copied());
        let first = it.next()?;
        let (lo, hi) = it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if (hi - lo).abs() < f64::EPSILON { Some((lo - 1.0, hi + 1.0)) } else { Some((lo, hi)) }
    }
}

/// The time of day, as the monitor shows it next to each line: `14:03:27.512`.
pub fn clock() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    // SAFETY: localtime_r writes only into the struct it's given.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    format!("{:02}:{:02}:{:02}.{:03}", tm.tm_hour, tm.tm_min, tm.tm_sec, now.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_end_however_the_board_ends_them() {
        let mut d = Decoder::default();
        assert_eq!(d.feed(b"um\r\ndois\nt"), ["um", "dois"]);
        assert_eq!(d.partial(), "t");
        // A \r\n split between two reads is one end of line, not two.
        assert_eq!(d.feed(b"res\r"), ["tres"]);
        assert_eq!(d.feed(b"\nquatro\rcinco\n"), ["quatro", "cinco"]);
        assert_eq!(d.feed(b"\n"), [""]);
        // Not UTF-8, and a stray escape: still shown.
        assert_eq!(d.feed(b"temp \xff\x1b ok\n"), ["temp \u{fffd} ok"]);
        // A character split between reads comes out whole.
        assert!(d.feed("á".as_bytes().get(..1).unwrap()).is_empty());
        assert_eq!(d.feed(&"á\n".as_bytes()[1..]), ["á"]);
    }

    #[test]
    fn events_from_the_bridge() {
        assert_eq!(parse_event("@open /dev/ttyACM0"), Some(PortEvent::Open("/dev/ttyACM0".into())));
        assert_eq!(parse_event("@lost\n"), Some(PortEvent::Lost));
        assert_eq!(parse_event("@none"), Some(PortEvent::NotFound));
        assert_eq!(parse_event("stty: erro"), None);
        assert_eq!(LineEnding::CrLf.bytes(), b"\r\n");
    }

    #[test]
    fn numbers_for_the_plotter() {
        assert_eq!(parse_values("1 2.5\t-3"), vec![("1".into(), 1.0), ("2".into(), 2.5), ("3".into(), -3.0)]);
        assert_eq!(parse_values("temp:21.5, hum=40"), vec![("temp".into(), 21.5), ("hum".into(), 40.0)]);
        assert!(parse_values("Olá! 3").is_empty(), "words aren't data");
        assert!(parse_values("").is_empty());
        let mut p = Plot { width: 3, ..Plot::default() };
        assert!(p.push_line("a:1"));
        assert!(p.push_line("a:2 b:10"));
        assert!(!p.push_line("conectado"));
        p.push_line("b:20");
        p.push_line("a:4");
        let a: Vec<f64> = p.series[0].1.iter().copied().collect();
        let b: Vec<f64> = p.series[1].1.iter().copied().collect();
        assert_eq!(a, [2.0, 2.0, 4.0], "a repeats while missing, and only the last 3 stay");
        assert_eq!(b, [10.0, 20.0, 20.0]);
        assert_eq!(p.range(), Some((2.0, 20.0)));
        p.clear();
        assert_eq!(p.range(), None);
    }

    #[test]
    fn the_clock_reads_like_a_clock() {
        let c = clock();
        assert_eq!(c.len(), 12, "{c}");
        assert_eq!(&c[2..3], ":");
        assert_eq!(&c[8..9], ".");
    }

    /// The bridge against a pseudo-terminal standing in for the board: what the board writes
    /// comes out, what goes in reaches the board, and the port's state is told.
    #[test]
    fn the_bridge_carries_bytes_both_ways() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let board = crate::dap::open_tty().unwrap();
        let port = board.path.to_string_lossy().into_owned();
        let argv = bridge_argv(Some(&port), 115_200);
        let mut child = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .process_group(0)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut events = BufReader::new(child.stderr.take().unwrap()).lines();
        assert_eq!(parse_event(&events.next().unwrap().unwrap()), Some(PortEvent::Waiting));
        assert_eq!(parse_event(&events.next().unwrap().unwrap()), Some(PortEvent::Open(port.clone())));
        let mut master = std::fs::File::from(board.master.try_clone().unwrap());
        unsafe {
            let fd = master.as_raw_fd();
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        master.write_all(b"leitura: 42\r\n").unwrap();
        let mut out = child.stdout.take().unwrap();
        let mut got = [0u8; 13];
        out.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"leitura: 42\r\n");
        child.stdin.as_mut().unwrap().write_all(b"LED 1\r\n").unwrap();
        let mut seen = Vec::new();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < end && !seen.ends_with(b"LED 1\r\n") {
            let mut buf = [0u8; 64];
            if let Ok(n) = master.read(&mut buf) {
                seen.extend_from_slice(&buf[..n]);
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(seen.ends_with(b"LED 1\r\n"), "the board got {seen:?}");
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGTERM);
        }
        let status = child.wait().unwrap();
        assert!(status.success() || status.code().is_none(), "{status:?}");
    }

    #[test]
    fn no_port_is_said_and_ends_the_bridge() {
        let argv = bridge_argv(Some("/dev/tinta-nao-existe"), 9600);
        // Waiting ten seconds is too long for a test: one attempt, by hand.
        let script = argv[2].replace("wait_port 100", "wait_port 1");
        let out = std::process::Command::new("sh").args(["-c", &script, "x", &argv[4], &argv[5]]).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.lines().any(|l| parse_event(l) == Some(PortEvent::NotFound)), "{err}");
    }
}
