//! A nested cosmic-comp on the X11 backend is an ordinary X window, so XTEST input to it goes
//! through its input stack and reaches the Wayland client as real pointer events; no compositor
//! offers a virtual pointer, so this is the only way to click a menu in a test. It has to be a
//! rootful Xwayland of its own (`Xwayland :77`, without `-rootless`): in a normal rootless one the host decides
//! which window the pointer is over. The desktop's own cursor doesn't move. Reading the window's
//! pixels shows what's on screen, not what the app thinks it drew.
//!
//!   tinta-xptr TITLE < steps
//!
//! Steps; coordinates are relative to the window:
//!   wait 200            pause in milliseconds
//!   move 100 50         move the pointer
//!   click 100 50        move and left click
//!   right 100 50        move and right click
//!   down / up           press or release the left button where the pointer is
//!   wheel up|down       one wheel notch
//!   shot out.png        save the window's pixels
//!   list                print the top-level windows

use std::io::BufRead;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, Window};
use x11rb::protocol::xtest::ConnectionExt as _;

const MOTION_NOTIFY: u8 = 6;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;

fn title<C: Connection>(c: &C, w: Window) -> String {
    let net = c.intern_atom(false, b"_NET_WM_NAME").ok().and_then(|x| x.reply().ok()).map(|r| r.atom);
    for atom in [net.unwrap_or(0), AtomEnum::WM_NAME.into()] {
        let Ok(cookie) = c.get_property(false, w, atom, AtomEnum::ANY, 0, 256) else { continue };
        if let Ok(reply) = cookie.reply()
            && !reply.value.is_empty()
        {
            return String::from_utf8_lossy(&reply.value).into_owned();
        }
    }
    String::new()
}

fn windows<C: Connection>(c: &C, root: Window, out: &mut Vec<(Window, String)>) {
    let Ok(Ok(tree)) = c.query_tree(root).map(|x| x.reply()) else { return };
    for w in tree.children {
        out.push((w, title(c, w)));
        windows(c, w, out);
    }
}

fn main() {
    let wanted = std::env::args().nth(1).unwrap_or_default();
    let (conn, screen) = x11rb::connect(None).expect("X11");
    let root = conn.setup().roots[screen].root;
    // Looked up on each step: the window may appear after this program starts.
    let scan = || {
        let mut all = Vec::new();
        windows(&conn, root, &mut all);
        all
    };

    let fake = |kind: u8, detail: u8, x: i16, y: i16| {
        conn.xtest_fake_input(kind, detail, 0, root, x, y, 0).unwrap();
        conn.flush().unwrap();
    };
    let at = |w: Window, x: i16, y: i16| {
        let r = conn.translate_coordinates(w, root, 0, 0).unwrap().reply().unwrap();
        fake(MOTION_NOTIFY, 0, r.dst_x + x, r.dst_y + y);
        std::thread::sleep(Duration::from_millis(30));
    };
    let button = |b: u8, down: bool| {
        fake(if down { BUTTON_PRESS } else { BUTTON_RELEASE }, b, 0, 0);
        std::thread::sleep(Duration::from_millis(25));
    };
    let press = |b: u8| {
        button(b, true);
        button(b, false);
    };

    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let word = it.next().unwrap();
        let rest: Vec<&str> = it.collect();
        let num = |i: usize| rest.get(i).and_then(|s| s.parse::<i16>().ok()).unwrap_or(0);
        if word == "list" {
            for (w, t) in &scan() {
                let g = conn.get_geometry(*w).unwrap().reply().unwrap();
                println!("{w:#x} {}x{} {t:?}", g.width, g.height);
            }
            continue;
        }
        if word == "wait" {
            std::thread::sleep(Duration::from_millis(rest.first().and_then(|s| s.parse().ok()).unwrap_or(100)));
            continue;
        }
        let found = scan().iter().filter(|(_, t)| !t.is_empty() && t.contains(&wanted)).map(|(w, _)| *w).next_back();
        let w = found.unwrap_or_else(|| panic!("no window titled {wanted:?}"));
        match word {
            "move" => at(w, num(0), num(1)),
            "click" => {
                at(w, num(0), num(1));
                press(1);
            }
            "right" => {
                at(w, num(0), num(1));
                press(3);
            }
            "middle" => {
                at(w, num(0), num(1));
                press(2);
            }
            // X11's keycode: evdev's plus 8. `key 57` is n on a US layout, `key 36` is Enter.
            "key" => {
                fake(2, num(0) as u8, 0, 0);
                fake(3, num(0) as u8, 0, 0);
                std::thread::sleep(Duration::from_millis(30));
            }
            "down" => button(1, true),
            "up" => button(1, false),
            "wheel" => press(if rest.first() == Some(&"up") { 4 } else { 5 }),
            "shot" => {
                let g = conn.get_geometry(w).unwrap().reply().unwrap();
                let img = conn
                    .get_image(ImageFormat::Z_PIXMAP, w, 0, 0, g.width, g.height, !0)
                    .unwrap()
                    .reply()
                    .expect("get_image");
                // BGRX on every X server we care about.
                let mut rgba = Vec::with_capacity(img.data.len());
                for p in img.data.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
                image::RgbaImage::from_raw(g.width as u32, g.height as u32, rgba).unwrap().save(rest[0]).unwrap();
            }
            other => panic!("unknown step {other}"),
        }
    }
    conn.flush().ok();
}
