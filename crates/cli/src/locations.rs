use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub label: String,
    pub path: String,
    /// `folder`, `volume` or `sd`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Locations {
    pub folders: Vec<Location>,
    pub volumes: Vec<Location>,
}

/// Common folders plus mounted volumes. Camera cards are tagged `sd` when
/// they contain a `DCIM` directory.
pub fn list() -> Locations {
    let mut folders = Vec::new();
    let mut volumes = Vec::new();

    if let Some(home) = home_dir() {
        push_dir(&mut folders, "个人目录", &home, "folder");
        for (name, label) in [
            ("Desktop", "桌面"),
            ("Downloads", "下载"),
            ("Movies", "影片"),
            ("Pictures", "图片"),
            ("Documents", "文稿"),
        ] {
            push_dir(&mut folders, label, &home.join(name), "folder");
        }
    }
    if let Ok(current) = std::env::current_dir() {
        let duplicate = folders
            .iter()
            .any(|location| Path::new(&location.path) == current);
        if !duplicate {
            push_dir(&mut folders, "当前目录", &current, "folder");
        }
    }

    #[cfg(not(windows))]
    {
        for root in volume_roots() {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                push_volume(&mut volumes, &entry.path());
            }
        }
    }
    #[cfg(windows)]
    {
        for letter in b'C'..=b'Z' {
            push_volume(
                &mut volumes,
                &PathBuf::from(format!("{}:\\", letter as char)),
            );
        }
    }
    volumes.sort_by(|left, right| left.label.cmp(&right.label));
    volumes.dedup_by(|left, right| left.path == right.path);

    Locations { folders, volumes }
}

fn push_volume(volumes: &mut Vec<Location>, path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() || !path.is_dir() {
        return;
    }
    // `/Volumes/Macintosh HD` is a link to the boot volume; skip it.
    if fs::canonicalize(path)
        .ok()
        .is_some_and(|canonical| canonical == Path::new("/"))
    {
        return;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    if name.starts_with('.') {
        return;
    }
    let is_card = path.join("DCIM").is_dir();
    volumes.push(Location {
        label: if is_card {
            format!("SD 卡 · {name}")
        } else {
            name.to_string()
        },
        path: path.display().to_string(),
        kind: if is_card { "sd" } else { "volume" }.to_string(),
    });
}

fn push_dir(folders: &mut Vec<Location>, label: &str, path: &Path, kind: &str) {
    if !path.is_dir() {
        return;
    }
    let value = path.display().to_string();
    if folders.iter().any(|location| location.path == value) {
        return;
    }
    folders.push(Location {
        label: label.to_string(),
        path: value,
        kind: kind.to_string(),
    });
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var_os(key).map(PathBuf::from)
}

#[cfg(not(windows))]
fn volume_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/Volumes")]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut roots = vec![PathBuf::from("/mnt")];
        if let Some(home) = home_dir() {
            let user = home
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            if !user.is_empty() {
                roots.push(PathBuf::from("/media").join(&user));
                roots.push(PathBuf::from("/run/media").join(user));
            }
        }
        roots
    }
}

/// Open the operating system's folder picker. `Ok(None)` means the user
/// cancelled.
pub fn pick_folder() -> Result<Option<PathBuf>> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"选择素材文件夹\" default location (POSIX file \"/Volumes\"))",
            ])
            .output()
            .context("run osascript (folder picker)")?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        if stderr.contains("-128") || stderr.contains("cancel") {
            return Ok(None);
        }
        bail!(
            "folder picker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    #[cfg(windows)]
    {
        let script = "Add-Type -AssemblyName System.Windows.Forms; \
                      $dialog = New-Object System.Windows.Forms.FolderBrowserDialog; \
                      $dialog.Description = 'Select footage folder'; \
                      if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { \
                        Write-Output $dialog.SelectedPath \
                      } else { exit 2 }";
        let output = Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", script])
            .output()
            .context("run powershell (folder picker)")?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
        }
        if output.status.code() == Some(2) {
            return Ok(None);
        }
        bail!(
            "folder picker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = run_linux_picker("zenity", &["--file-selection", "--directory"])? {
            return Ok(Some(path));
        }
        if let Some(path) = run_linux_picker("kdialog", &["--getexistingdirectory"])? {
            return Ok(Some(path));
        }
        bail!("no folder picker found; install zenity or kdialog");
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_linux_picker(program: &str, args: &[&str]) -> Result<Option<PathBuf>> {
    let output = match Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context(format!("run {program}")),
    };
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
    }
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    bail!(
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_camera_cards_by_dcim() {
        let root = std::env::temp_dir().join(format!(
            "shotrename-locations-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let card = root.join("EOS_DIGITAL");
        std::fs::create_dir_all(card.join("DCIM")).unwrap();
        let mut volumes = Vec::new();
        push_volume(&mut volumes, &card);
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].kind, "sd");
        assert!(volumes[0].label.starts_with("SD 卡"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lists_home_and_existing_folders_only() {
        let locations = list();
        for folder in &locations.folders {
            assert!(Path::new(&folder.path).is_dir(), "{}", folder.path);
            assert!(!folder.label.is_empty());
        }
        if let Some(home) = home_dir() {
            assert!(locations
                .folders
                .iter()
                .any(|folder| Path::new(&folder.path) == home));
        }
    }
}
