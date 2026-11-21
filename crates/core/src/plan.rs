use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlanStatus {
    Ready,
    Blocked,
    /// The file already has the rendered name; nothing to do.
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedMove {
    pub source: PathBuf,
    pub target: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEntry {
    pub source: PathBuf,
    pub target: PathBuf,
    #[serde(default)]
    pub related: Vec<RelatedMove>,
    pub status: PlanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    pub template: String,
    pub entries: Vec<PlanEntry>,
}

impl Plan {
    pub const VERSION: u32 = 1;

    pub fn ready_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status == PlanStatus::Ready)
            .count()
    }

    pub fn blocked_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status == PlanStatus::Blocked)
            .count()
    }

    pub fn unchanged_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status == PlanStatus::Unchanged)
            .count()
    }
}

/// Allocates unique file names inside a plan.
///
/// Names are compared case-insensitively for every platform so the same plan
/// behaves the same on macOS, Windows and Linux. Existing files are reserved
/// before planning starts.
#[derive(Debug, Default)]
pub struct NameAllocator {
    reserved: HashMap<PathBuf, HashSet<String>>,
}

impl NameAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve_existing(&mut self, path: &Path) {
        let Some(parent) = path.parent() else {
            return;
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        self.reserved
            .entry(parent.to_path_buf())
            .or_default()
            .insert(name.to_lowercase());
    }

    pub fn is_taken(&self, dir: &Path, file_name: &str) -> bool {
        if dir.join(file_name).exists() {
            return true;
        }
        self.reserved
            .get(dir)
            .is_some_and(|names| names.contains(&file_name.to_lowercase()))
    }

    /// Allocate `stem[_NNN].ext` in `dir`, reserving the result.
    pub fn allocate(&mut self, dir: &Path, stem: &str, extension: &str) -> PathBuf {
        let extension = extension.trim_start_matches('.');
        let file_name = if extension.is_empty() {
            format!("{stem}")
        } else {
            format!("{stem}.{extension}")
        };
        if !self.is_taken(dir, &file_name) {
            self.reserve(dir, &file_name);
            return dir.join(file_name);
        }
        for counter in 1..=999 {
            let candidate = if extension.is_empty() {
                format!("{stem}_{counter:03}")
            } else {
                format!("{stem}_{counter:03}.{extension}")
            };
            if !self.is_taken(dir, &candidate) {
                self.reserve(dir, &candidate);
                return dir.join(candidate);
            }
        }
        // Exhausted the numeric space; return a highly unlikely name instead of
        // silently overwriting. Planning should block this entry before apply.
        let fallback = if extension.is_empty() {
            format!("{stem}_overflow")
        } else {
            format!("{stem}_overflow.{extension}")
        };
        self.reserve(dir, &fallback);
        dir.join(fallback)
    }

    fn reserve(&mut self, dir: &Path, file_name: &str) {
        self.reserved
            .entry(dir.to_path_buf())
            .or_default()
            .insert(file_name.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn appends_three_digit_suffix() {
        let temp = std::env::temp_dir().join(format!("shotrename-plan-{}", std::process::id()));
        let _ = fs::create_dir_all(&temp);
        fs::write(temp.join("clip.MP4"), b"x").unwrap();

        let mut allocator = NameAllocator::new();
        allocator.reserve_existing(&temp.join("clip.MP4"));
        let target = allocator.allocate(&temp, "clip", "MP4");
        assert_eq!(target.file_name().unwrap(), "clip_001.MP4");

        let next = allocator.allocate(&temp, "clip", "MP4");
        assert_eq!(next.file_name().unwrap(), "clip_002.MP4");
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn is_case_insensitive() {
        let temp = std::env::temp_dir().join(format!("shotrename-case-{}", std::process::id()));
        let _ = fs::create_dir_all(&temp);
        fs::write(temp.join("Clip.MP4"), b"x").unwrap();
        let mut allocator = NameAllocator::new();
        allocator.reserve_existing(&temp.join("Clip.MP4"));
        let target = allocator.allocate(&temp, "clip", "mp4");
        assert_eq!(target.file_name().unwrap(), "clip_001.mp4");
        let _ = fs::remove_dir_all(&temp);
    }
}
