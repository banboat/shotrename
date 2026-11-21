use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub source: PathBuf,
    pub target: PathBuf,
    pub size: u64,
    #[serde(default)]
    pub related: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub version: u32,
    pub batch_id: String,
    pub created_at: String,
    pub operations: Vec<Operation>,
}

impl JournalEntry {
    pub const VERSION: u32 = 1;
}

pub fn append(path: &Path, entry: &JournalEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(entry).context("serialize journal entry")?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open journal {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("write journal {}", path.display()))?;
    file.sync_all().context("sync journal")?;
    Ok(())
}

pub fn last(path: &Path) -> Result<Option<JournalEntry>> {
    if !path.exists() {
        return Ok(None);
    }
    let data =
        fs::read_to_string(path).with_context(|| format!("read journal {}", path.display()))?;
    let Some(line) = data.lines().rev().find(|line| !line.trim().is_empty()) else {
        return Ok(None);
    };
    let entry = serde_json::from_str(line).context("parse last journal entry")?;
    Ok(Some(entry))
}

/// Drop the most recent batch after a successful undo.
pub fn remove_last(path: &Path) -> Result<()> {
    let data =
        fs::read_to_string(path).with_context(|| format!("read journal {}", path.display()))?;
    let mut lines: Vec<&str> = data.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.pop();
    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    fs::write(path, output).with_context(|| format!("rewrite journal {}", path.display()))?;
    Ok(())
}

pub fn count(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let data =
        fs::read_to_string(path).with_context(|| format!("read journal {}", path.display()))?;
    Ok(data.lines().filter(|line| !line.trim().is_empty()).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_last_entry() {
        let path = std::env::temp_dir().join(format!(
            "shotrename-journal-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let entry = JournalEntry {
            version: JournalEntry::VERSION,
            batch_id: "b1".into(),
            created_at: "2026-11-21T14:30:15+08:00".into(),
            operations: vec![Operation {
                source: PathBuf::from("/a/IMG_0001.MP4"),
                target: PathBuf::from("/a/2026-11-21_IMG_0001.MP4"),
                size: 12,
                related: false,
            }],
        };
        append(&path, &entry).unwrap();
        let loaded = last(&path).unwrap().unwrap();
        assert_eq!(loaded.batch_id, "b1");
        assert_eq!(loaded.operations.len(), 1);
        assert_eq!(count(&path).unwrap(), 1);
        let _ = fs::remove_file(path);
    }
}
