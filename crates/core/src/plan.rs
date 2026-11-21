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
        for (candidate, is_fallback) in candidates(stem) {
            let file_name = compose_name(&candidate, "", extension);
            if is_fallback || !self.is_taken(dir, &file_name) {
                self.reserve(dir, &file_name);
                return dir.join(file_name);
            }
        }
        unreachable!("candidates always yields a fallback")
    }

    /// Allocate one shared base for a file group, reserving every member name.
    ///
    /// `members` holds `(suffix, extension)` pairs. `owned` lists existing
    /// paths that belong to the group itself; those may sit on a candidate
    /// target without counting as a conflict, which keeps partially renamed
    /// groups stable.
    pub fn allocate_group(
        &mut self,
        dir: &Path,
        stem: &str,
        members: &[(String, String)],
        owned: &[PathBuf],
    ) -> String {
        let members: Vec<(String, String)> = members
            .iter()
            .map(|(suffix, extension)| {
                (
                    suffix.clone(),
                    extension.trim_start_matches('.').to_string(),
                )
            })
            .collect();
        for (candidate, is_fallback) in candidates(stem) {
            let names: Vec<String> = members
                .iter()
                .map(|(suffix, extension)| compose_name(&candidate, suffix, extension))
                .collect();
            let free = is_fallback
                || names.iter().all(|name| {
                    let path = dir.join(name);
                    !self.is_taken(dir, name) || owned.contains(&path)
                });
            if free {
                for name in &names {
                    self.reserve(dir, name);
                }
                return candidate;
            }
        }
        unreachable!("candidates always yields a fallback")
    }

    fn reserve(&mut self, dir: &Path, file_name: &str) {
        self.reserved
            .entry(dir.to_path_buf())
            .or_default()
            .insert(file_name.to_lowercase());
    }
}

/// Candidate base stems in allocation order, ending with a fallback that is
/// always accepted so planning never silently overwrites.
fn candidates(stem: &str) -> impl Iterator<Item = (String, bool)> {
    let stem = stem.to_string();
    let overflow = format!("{stem}_overflow");
    std::iter::once((stem.clone(), false))
        .chain((1..=999).map(move |counter| (format!("{stem}_{counter:03}"), false)))
        .chain(std::iter::once((overflow, true)))
}

fn compose_name(base: &str, suffix: &str, extension: &str) -> String {
    if extension.is_empty() {
        format!("{base}{suffix}")
    } else {
        format!("{base}{suffix}.{extension}")
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
    fn allocates_one_base_for_a_group() {
        let temp = std::env::temp_dir().join(format!("shotrename-group-{}", std::process::id()));
        let _ = fs::create_dir_all(&temp);
        fs::write(temp.join("clip_00.insv"), b"x").unwrap();

        let mut allocator = NameAllocator::new();
        let members = vec![("_00".to_string(), "insv".to_string())];
        let owned = vec![temp.join("clip_00.insv")];
        // The group already owns the conflicting name, so the base is kept.
        let base = allocator.allocate_group(&temp, "clip", &members, &owned);
        assert_eq!(base, "clip");

        let members = vec![
            ("_00".to_string(), "insv".to_string()),
            ("_10".to_string(), "insv".to_string()),
        ];
        let base = allocator.allocate_group(&temp, "clip", &members, &[]);
        assert_eq!(base, "clip_001");
        assert!(allocator.is_taken(&temp, "clip_001_00.insv"));
        assert!(allocator.is_taken(&temp, "clip_001_10.insv"));
        let _ = fs::remove_dir_all(temp);
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
