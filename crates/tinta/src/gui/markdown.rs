//! Rendered Markdown in a tab of its own, kept in step with the text it shows.

use super::Shell;
use crate::state::TabId;
use crate::ui::markdown::{Block, Inline};
use adw::prelude::*;
use std::rc::Rc;

pub fn view(_shell: &Rc<Shell>, _tab: TabId) -> gtk::ScrolledWindow {
    let tv = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(32)
        .right_margin(32)
        .top_margin(24)
        .bottom_margin(24)
        .pixels_below_lines(4)
        .build();
    let buf = tv.buffer();
    buf.create_tag(Some("h1"), &[("scale", &2.0f64), ("weight", &800)]);
    buf.create_tag(Some("h2"), &[("scale", &1.6f64), ("weight", &800)]);
    buf.create_tag(Some("h3"), &[("scale", &1.3f64), ("weight", &700)]);
    buf.create_tag(Some("h4"), &[("scale", &1.1f64), ("weight", &700)]);
    buf.create_tag(Some("bold"), &[("weight", &700)]);
    buf.create_tag(Some("italic"), &[("style", &gtk::pango::Style::Italic)]);
    buf.create_tag(Some("strike"), &[("strikethrough", &true)]);
    buf.create_tag(Some("code"), &[("family", &"monospace"), ("background-rgba", &gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 0.15))]);
    buf.create_tag(Some("link"), &[("underline", &gtk::pango::Underline::Single), ("foreground-rgba", &adw::StyleManager::default().accent_color_rgba())]);
    buf.create_tag(Some("quote"), &[("left-margin", &56), ("style", &gtk::pango::Style::Italic), ("foreground-rgba", &gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0))]);
    buf.create_tag(Some("block"), &[("family", &"monospace"), ("left-margin", &48), ("paragraph-background-rgba", &gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 0.10))]);
    for d in 0..6 {
        buf.create_tag(Some(&format!("item{d}")), &[("left-margin", &(48 + d * 22))]);
    }
    gtk::ScrolledWindow::builder().child(&tv).vexpand(true).hexpand(true).build()
}

fn inlines(buf: &gtk::TextBuffer, items: &[Inline], extra: &[&str]) {
    for it in items {
        let mut tags: Vec<&str> = extra.to_vec();
        if it.bold {
            tags.push("bold");
        }
        if it.italic {
            tags.push("italic");
        }
        if it.code {
            tags.push("code");
        }
        if it.link {
            tags.push("link");
        }
        if it.strike {
            tags.push("strike");
        }
        let mut end = buf.end_iter();
        buf.insert_with_tags_by_name(&mut end, &it.text, &tags);
    }
}

/// Redraws the page when the text it shows has changed.
pub fn refresh(page: &adw::TabPage, shell: &Rc<Shell>, tab: TabId) {
    let Some(overlay) = page.child().downcast::<gtk::Overlay>().ok() else { return };
    let Some(scroll) = overlay.child().and_downcast::<gtk::ScrolledWindow>() else { return };
    let Some(tv) = scroll.child().and_downcast::<gtk::TextView>() else { return };
    let Ok(app) = shell.app.try_borrow() else { return };
    let Some((rev, blocks)) = app.tabs.get(&tab).and_then(|t| t.preview_cache.as_ref()) else { return };
    let key = rev.to_string();
    if tv.widget_name() == key {
        return;
    }
    tv.set_widget_name(&key);
    let buf = tv.buffer();
    buf.set_text("");
    for b in blocks {
        let mut end = buf.end_iter();
        match b {
            Block::Heading(level, items) => {
                let tag = format!("h{}", (*level).clamp(1, 4));
                inlines(&buf, items, &[tag.as_str()]);
            }
            Block::Para(items) => inlines(&buf, items, &[]),
            Block::Code(text) => buf.insert_with_tags_by_name(&mut end, text.trim_end(), &["block"]),
            Block::Item { depth, marker, content } => {
                let tag = format!("item{}", (*depth).min(5));
                buf.insert_with_tags_by_name(&mut end, &format!("{marker} "), &[tag.as_str()]);
                inlines(&buf, content, &[tag.as_str()]);
            }
            Block::Quote(items) => inlines(&buf, items, &["quote"]),
            Block::Rule => buf.insert(&mut end, "──────────────"),
            Block::Row { cells, header } => {
                for (i, c) in cells.iter().enumerate() {
                    if i > 0 {
                        let mut end = buf.end_iter();
                        buf.insert(&mut end, "   │   ");
                    }
                    inlines(&buf, c, if *header { &["bold"] } else { &[] });
                }
            }
        }
        let mut end = buf.end_iter();
        buf.insert(&mut end, "\n");
    }
}
