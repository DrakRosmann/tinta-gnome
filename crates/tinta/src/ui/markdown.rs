//! Markdown parsed into blocks, for the rendered preview.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Default)]
pub struct Inline {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
    pub strike: bool,
}

#[derive(Debug, Clone)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Para(Vec<Inline>),
    Code(String),
    Item { depth: usize, marker: String, content: Vec<Inline> },
    Quote(Vec<Inline>),
    Rule,
    Row { cells: Vec<Vec<Inline>>, header: bool },
}

pub fn parse(src: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = Vec::new();
    let mut cur: Vec<Inline> = Vec::new();
    let (mut bold, mut italic, mut link, mut strike) = (0, 0, 0, 0);
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut in_quote = 0;
    let mut code: Option<String> = None;
    let mut cells: Vec<Vec<Inline>> = Vec::new();
    let mut in_head = false;
    let push_text = |cur: &mut Vec<Inline>, t: &str, code: bool, bold: i32, italic: i32, link: i32, strike: i32| {
        cur.push(Inline {
            text: t.to_string(),
            bold: bold > 0,
            italic: italic > 0,
            code,
            link: link > 0,
            strike: strike > 0,
        });
    };
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold -= 1,
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic -= 1,
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike -= 1,
            Event::Start(Tag::Link { .. }) => link += 1,
            Event::End(TagEnd::Link) => link -= 1,
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(c) = code.take() {
                    out.push(Block::Code(c.trim_end().to_string()));
                }
            }
            Event::Start(Tag::List(first)) => lists.push(first),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::BlockQuote(_)) => in_quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => in_quote -= 1,
            Event::End(TagEnd::Heading(level)) => {
                let l = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    _ => 5,
                };
                out.push(Block::Heading(l, std::mem::take(&mut cur)));
            }
            Event::End(TagEnd::Paragraph) => {
                if lists.is_empty() {
                    let c = std::mem::take(&mut cur);
                    out.push(if in_quote > 0 { Block::Quote(c) } else { Block::Para(c) });
                }
            }
            Event::Start(Tag::Item) => {
                cur.clear();
            }
            Event::End(TagEnd::Item) => {
                let depth = lists.len().saturating_sub(1);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "•".to_string(),
                };
                let content = std::mem::take(&mut cur);
                out.push(Block::Item { depth, marker, content });
            }
            Event::TaskListMarker(done) => {
                cur.insert(0, Inline { text: if done { "☑ ".into() } else { "☐ ".into() }, ..Default::default() });
            }
            Event::Start(Tag::TableHead) => {
                in_head = true;
                cells.clear();
            }
            Event::End(TagEnd::TableHead) => {
                out.push(Block::Row { cells: std::mem::take(&mut cells), header: true });
                in_head = false;
            }
            Event::End(TagEnd::TableRow) => {
                out.push(Block::Row { cells: std::mem::take(&mut cells), header: in_head });
            }
            Event::End(TagEnd::TableCell) => cells.push(std::mem::take(&mut cur)),
            Event::Text(t) => {
                if let Some(c) = &mut code {
                    c.push_str(&t);
                } else {
                    push_text(&mut cur, &t, false, bold, italic, link, strike);
                }
            }
            Event::Code(t) => push_text(&mut cur, &t, true, bold, italic, link, strike),
            Event::SoftBreak => push_text(&mut cur, " ", false, bold, italic, link, strike),
            Event::HardBreak => push_text(&mut cur, "\n", false, bold, italic, link, strike),
            Event::Rule => out.push(Block::Rule),
            Event::Html(t) | Event::InlineHtml(t) => push_text(&mut cur, &t, true, 0, 0, 0, 0),
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push(Block::Para(cur));
    }
    out
}
