use serde::Serialize;
use std::path::Path;

use crate::detect::extension;

/// Multi-file shots that must be renamed together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    /// Insta360 360 footage: one `.insv` per lens.
    Insta360Lens,
    /// GoPro chaptered recordings: `GOPR0001`, `GP010001`, `GX010001`.
    GoProChapter,
}

impl GroupKind {
    pub fn label(self) -> &'static str {
        match self {
            GroupKind::Insta360Lens => "insta360-lens",
            GroupKind::GoProChapter => "gopro-chapter",
        }
    }
}

/// How one file participates in its group.
#[derive(Debug, Clone)]
pub struct GroupMember {
    pub kind: GroupKind,
    /// Group identity inside one directory.
    pub key: String,
    /// Sort order inside the group: lens number, or chapter rank.
    pub rank: u32,
    /// Insta360 lens token (`00` front, `10` back), empty otherwise.
    pub lens: String,
    /// True when the name carries an explicit chapter prefix (`GX`/`GP`).
    pub chaptered: bool,
}

impl GroupMember {
    /// Suffix appended to the shared base stem, if this member needs one.
    ///
    /// `chapter` is the unique 1-based chapter number computed by
    /// [`chapter_labels`]. Insta360 always keeps its lens token; GoPro adds
    /// `_chNN` for chaptered names and for any first-chapter `GOPR` file
    /// that has siblings.
    pub fn suffix(&self, chapter: u32, group_len: usize) -> Option<String> {
        match self.kind {
            GroupKind::Insta360Lens => Some(format!("_{}", self.lens)),
            GroupKind::GoProChapter => {
                if self.chaptered || group_len > 1 {
                    Some(format!("_ch{chapter:02}"))
                } else {
                    None
                }
            }
        }
    }
}

/// Unique, increasing chapter numbers in member order.
///
/// `GP01` is GoPro's second chapter (after `GOPR`), so its rank-based label
/// is 2 even when the first chapter is missing. Labels never repeat, which
/// keeps `GOPR0001` + `GX010001` from targeting the same name.
pub fn chapter_labels(members: &[GroupMember]) -> Vec<u32> {
    let mut next = 1;
    members
        .iter()
        .map(|member| {
            let label = (member.rank + 1).max(next);
            next = label + 1;
            label
        })
        .collect()
}

/// Classify a file as a group member by its name. Returns `None` for
/// ordinary single-file shots.
pub fn classify(path: &Path) -> Option<GroupMember> {
    let ext = extension(path);
    let stem = path.file_stem()?.to_str()?;
    if ext == "insv" {
        return parse_insta360(stem);
    }
    if matches!(ext.as_str(), "mp4" | "mov" | "lrv" | "thm") {
        return parse_gopro(stem);
    }
    None
}

fn parse_insta360(stem: &str) -> Option<GroupMember> {
    let upper = stem.to_ascii_uppercase();
    let rest = upper.strip_prefix("VID_")?;
    let parts: Vec<&str> = rest.split('_').collect();
    if parts.len() < 4 {
        return None;
    }
    let (date, time, lens, segment) = (parts[0], parts[1], parts[2], parts[3]);
    if date.len() != 8
        || time.len() != 6
        || lens.len() != 2
        || !date.bytes().all(|byte| byte.is_ascii_digit())
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || !lens.bytes().all(|byte| byte.is_ascii_digit())
        || segment.is_empty()
    {
        return None;
    }
    Some(GroupMember {
        kind: GroupKind::Insta360Lens,
        key: format!("{date}_{time}_{segment}"),
        rank: lens.parse().unwrap_or(0),
        lens: lens.to_string(),
        chaptered: false,
    })
}

fn parse_gopro(stem: &str) -> Option<GroupMember> {
    let upper = stem.to_ascii_uppercase();
    if let Some(id) = upper.strip_prefix("GOPR") {
        if id.len() == 4 && id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some(GroupMember {
                kind: GroupKind::GoProChapter,
                key: id.to_string(),
                rank: 0,
                lens: String::new(),
                chaptered: false,
            });
        }
        return None;
    }
    for prefix in ["GX", "GP"] {
        let Some(rest) = upper.strip_prefix(prefix) else {
            continue;
        };
        if rest.len() != 6 || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let chapter: u32 = rest[..2].parse().ok()?;
        let id = &rest[2..];
        // GX01 is chapter 1; GP01 follows GOPR and is chapter 2.
        let rank = if prefix == "GX" {
            chapter.saturating_sub(1)
        } else {
            chapter
        };
        return Some(GroupMember {
            kind: GroupKind::GoProChapter,
            key: id.to_string(),
            rank,
            lens: String::new(),
            chaptered: true,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn member(name: &str) -> Option<GroupMember> {
        classify(&PathBuf::from(name))
    }

    #[test]
    fn pairs_insta360_lenses() {
        let front = member("VID_20241121_143015_00_001.insv").unwrap();
        let back = member("VID_20241121_143015_10_001.insv").unwrap();
        assert_eq!(front.kind, GroupKind::Insta360Lens);
        assert_eq!(front.key, back.key);
        assert_eq!((front.rank, back.rank), (0, 10));
        assert_eq!(front.suffix(1, 2).as_deref(), Some("_00"));
        assert_eq!(back.suffix(2, 2).as_deref(), Some("_10"));
    }

    #[test]
    fn ignores_insta360_mp4() {
        assert!(member("VID_20241121_143015_00_001.mp4").is_none());
    }

    #[test]
    fn computes_gopro_chapters() {
        let first = member("GOPR0123.MP4").unwrap();
        let second = member("GP010123.MP4").unwrap();
        let third = member("GX020123.MP4").unwrap();
        assert_eq!(first.key, "0123");
        assert_eq!(second.key, "0123");
        assert_eq!(third.key, "0123");
        assert_eq!((first.rank, second.rank, third.rank), (0, 1, 1));
        assert_eq!(second.suffix(2, 3).as_deref(), Some("_ch02"));
        // A lone GOPR file needs no suffix; a chaptered name always keeps one.
        assert_eq!(first.suffix(1, 1), None);
        assert_eq!(third.suffix(1, 1).as_deref(), Some("_ch01"));
    }

    #[test]
    fn keeps_gopro_chapter_numbers_unique() {
        let members = [
            member("GOPR0123.MP4").unwrap(),
            member("GP010123.MP4").unwrap(),
            member("GX010123.MP4").unwrap(),
        ];
        let labels = chapter_labels(&members);
        assert_eq!(labels, vec![1, 2, 3]);
        // A duplicate rank is pushed forward instead of colliding.
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 3);
        // A lone GP01 is chapter 2, not chapter 1.
        let lone = [member("GP010123.MP4").unwrap()];
        assert_eq!(chapter_labels(&lone), vec![2]);
    }

    #[test]
    fn ignores_unrelated_names() {
        assert!(member("clip.mp4").is_none());
        assert!(member("GX01.MP4").is_none());
    }
}
