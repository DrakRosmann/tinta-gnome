//! What gets restored at startup: open files, cursors, layout and unsaved work.

use crate::cursor::Cursors;
use crate::display::Folds;
use crate::settings::state_dir;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TabState {
    pub path: Option<PathBuf>,
    pub cursors: Cursors,
    pub scroll_y: f32,
    pub folds: Folds,
    pub pinned: bool,
    /// Key of the saved copy of unsaved text, if any.
    pub backup: Option<String>,
    pub scratch_name: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupState {
    pub tabs: Vec<TabState>,
    pub active: usize,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub folder: Option<PathBuf>,
    pub groups: Vec<GroupState>,
    pub active_group: usize,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub panel_height: f32,
    pub recent_files: Vec<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub expanded: Vec<PathBuf>,
    #[serde(default)]
    pub breakpoints: crate::dap::Breakpoints,
    #[serde(default)]
    pub watches: Vec<String>,
    /// What "Run File with Arguments…" was last given, for each file.
    #[serde(default)]
    pub run_args: std::collections::HashMap<PathBuf, String>,
}

fn session_path() -> PathBuf {
    state_dir().join("session.json")
}

pub fn backups_dir() -> PathBuf {
    state_dir().join("backups")
}

impl Session {
    pub fn load() -> Option<Self> {
        Self::load_from(&session_path())
    }

    pub fn load_from(path: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&session_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        crate::buffer::atomic_write(path, text.as_bytes()).map_err(|e| e.to_string())
    }
}

/// Stable key for a buffer's backup: by path, or by a generated id for untitled buffers.
pub fn backup_key(path: Option<&Path>, id: u64) -> String {
    let mut h = DefaultHasher::new();
    match path {
        Some(p) => p.hash(&mut h),
        None => id.hash(&mut h),
    }
    format!("{:016x}", h.finish())
}

pub fn write_backup(key: &str, text: &str) -> Result<(), String> {
    let dir = backups_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::buffer::atomic_write(&dir.join(key), text.as_bytes()).map_err(|e| e.to_string())
}

pub fn read_backup(key: &str) -> Option<String> {
    std::fs::read_to_string(backups_dir().join(key)).ok()
}

pub fn remove_backup(key: &str) {
    let _ = std::fs::remove_file(backups_dir().join(key));
}

/// Deletes backups no tab refers to any more.
pub fn prune_backups(keep: &[String]) {
    let Ok(entries) = std::fs::read_dir(backups_dir()) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !keep.contains(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::Selection;

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("tinta-sess-{}", std::process::id()));
        let p = dir.join("s.json");
        let mut f = Folds::default();
        f.add(1, 3);
        let s = Session {
            folder: Some("/tmp/x".into()),
            groups: vec![GroupState {
                tabs: vec![TabState {
                    path: Some("/tmp/x/a.rs".into()),
                    cursors: Cursors::single(Selection::new(1, 2)),
                    scroll_y: 40.0,
                    folds: f,
                    ..Default::default()
                }],
                active: 0,
                weight: 1.0,
            }],
            ..Default::default()
        };
        s.save_to(&p).unwrap();
        let back = Session::load_from(&p).unwrap();
        assert_eq!(back.folder, s.folder);
        assert_eq!(back.groups[0].tabs[0].cursors.primary(), Selection::new(1, 2));
        assert_eq!(back.groups[0].tabs[0].folds.ranges, vec![(1, 3)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupted_session_is_ignored() {
        let dir = std::env::temp_dir().join(format!("tinta-sess2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.json");
        std::fs::write(&p, "{ not json").unwrap();
        assert!(Session::load_from(&p).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_keys_are_stable() {
        let a = backup_key(Some(Path::new("/x/y")), 1);
        assert_eq!(a, backup_key(Some(Path::new("/x/y")), 2));
        assert_ne!(backup_key(None, 1), backup_key(None, 2));
    }
}
