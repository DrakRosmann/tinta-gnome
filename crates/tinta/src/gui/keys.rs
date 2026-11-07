//! GTK's key events in the shape the app's keymap reads: a lowercase key name, what shift makes of
//! it, the modifiers and the text typed.

use crate::app::KeyEv;
use crate::gfx::Modifiers;
use gtk::gdk;
use gtk::gdk::prelude::*;

fn named(k: gdk::Key) -> Option<&'static str> {
    use gdk::Key as K;
    Some(match k {
        K::Return | K::KP_Enter | K::ISO_Enter => "enter",
        K::Tab | K::ISO_Left_Tab | K::KP_Tab => "tab",
        K::BackSpace => "backspace",
        K::Delete | K::KP_Delete => "delete",
        K::Insert | K::KP_Insert => "insert",
        K::Escape => "escape",
        K::Up | K::KP_Up => "up",
        K::Down | K::KP_Down => "down",
        K::Left | K::KP_Left => "left",
        K::Right | K::KP_Right => "right",
        K::Home | K::KP_Home => "home",
        K::End | K::KP_End => "end",
        K::Page_Up | K::KP_Page_Up => "pageup",
        K::Page_Down | K::KP_Page_Down => "pagedown",
        K::space | K::KP_Space => "space",
        K::F1 => "f1",
        K::F2 => "f2",
        K::F3 => "f3",
        K::F4 => "f4",
        K::F5 => "f5",
        K::F6 => "f6",
        K::F7 => "f7",
        K::F8 => "f8",
        K::F9 => "f9",
        K::F10 => "f10",
        K::F11 => "f11",
        K::F12 => "f12",
        _ => return None,
    })
}

/// Keys that only modify others: nothing to do on their own.
pub fn is_modifier(k: gdk::Key) -> bool {
    use gdk::Key as K;
    matches!(
        k,
        K::Shift_L | K::Shift_R | K::Control_L | K::Control_R | K::Alt_L | K::Alt_R | K::Super_L | K::Super_R | K::Meta_L | K::Meta_R | K::ISO_Level3_Shift | K::Caps_Lock | K::Num_Lock | K::Hyper_L | K::Hyper_R | K::ISO_Level5_Shift
    )
}

/// Keys that start an accent, like ´ and ~ on a Brazilian keyboard: the input method composes them.
pub fn is_dead(k: gdk::Key) -> bool {
    k.name().is_some_and(|n| n.starts_with("dead_") || n == "Multi_key")
}

fn char_of(k: gdk::Key) -> Option<char> {
    k.to_unicode().filter(|c| !c.is_control())
}

pub fn key_event(display: &gdk::Display, keyval: gdk::Key, keycode: u32, state: gdk::ModifierType) -> Option<KeyEv> {
    if is_modifier(keyval) {
        return None;
    }
    let mods = Modifiers::from_gdk(state);
    let mut shifted = None;
    let name = if let Some(n) = named(keyval) {
        n.to_string()
    } else {
        // The key without shift: `=` for the key that makes `+`.
        let base = display
            .translate_key(keycode, gdk::ModifierType::empty(), 0)
            .map(|(k, ..)| k)
            .and_then(char_of)
            .or_else(|| char_of(keyval))?;
        let lower: String = base.to_lowercase().collect();
        if let Some(actual) = char_of(keyval) {
            let a: String = actual.to_lowercase().collect();
            if a != lower {
                shifted = Some(a);
            }
        }
        // Layouts without Latin letters keep Ctrl+C on the key that has C in Latin layouts.
        if mods.control() && !lower.is_ascii() {
            display
                .translate_key(keycode, gdk::ModifierType::empty(), 0)
                .and_then(|(k, ..)| char_of(k))
                .map(|c| c.to_lowercase().collect())
                .unwrap_or(lower)
        } else {
            lower
        }
    };
    let text = if mods.control() || mods.alt() || mods.logo() {
        None
    } else {
        char_of(keyval).map(|c| c.to_string())
    };
    Some(KeyEv { name, shifted, mods, text, captured: false })
}
