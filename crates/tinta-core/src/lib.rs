//! Text engine of the Tinta editor. Nothing in here knows about windows: every
//! feature can be exercised from a unit test.

pub mod buffer;
pub mod commands;
pub mod cursor;
pub mod desktop;
pub mod dap;
pub mod diff;
pub mod display;
pub mod find;
pub mod fuzzy;
pub mod git;
pub mod host;
pub mod keymap;
pub mod keywords;
pub mod language;
pub mod lsp;
pub mod outline;
pub mod run;
pub mod session;
pub mod settings;
pub mod snippet;
pub mod structure;
pub mod syntax;
pub mod tasks;
pub mod vim;

pub use buffer::{Document, Edit, EditKind};
pub use cursor::{Cursors, Selection};
pub use display::DisplayMap;
