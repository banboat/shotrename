use chrono::{NaiveDate, NaiveDateTime};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Dji,
    Insta360,
    GoPro,
    Generic,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Dji => "DJI",
            Family::Insta360 => "Insta360",
            Family::GoPro => "GoPro",
            Family::Generic => "Camera",
        }
    }
}

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "avi", "webm", "ts", "mts", "m2ts", "wmv", "flv", "3gp", "3g2",
    "mxf", "mpg", "mpeg", "ogv", "qt", "m2v", "3gpp", "3gp2", "dv", "vob", "insv",
];

pub const RELATED_EXTENSIONS: &[&str] = &["lrf", "lrv", "srt", "wav", "thm"];

pub const TEMP_EXTENSIONS: &[&str] = &["crdownload", "part", "tmp"];

pub fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub fn is_supported_video(path: &Path) -> bool {
    VIDEO_EXTENSIONS.contains(&extension(path).as_str())
}

pub fn is_temporary(path: &Path) -> bool {
    TEMP_EXTENSIONS.contains(&extension(path).as_str())
}

pub fn is_excluded_dir(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "node_modules"
            | "target"
            | "dist"
            | "build"
            | "coverage"
            | "vendor"
            | "system volume information"
            | "$recycle.bin"
    )
}

pub fn detect_family(path: &Path) -> Family {
    let ext = extension(path);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    if ext == "insv" || upper.starts_with("VID_") || upper.starts_with("LRV_") {
        Family::Insta360
    } else if upper.starts_with("DJI_") {
        Family::Dji
    } else if upper.starts_with("GX")
        || upper.starts_with("GP")
        || upper.starts_with("GH")
        || upper.starts_with("GOPR")
        || upper.starts_with("GL")
    {
        Family::GoPro
    } else {
        Family::Generic
    }
}

/// Extract a capture time from common camera file name patterns.
///
/// Recognises `YYYYMMDDHHMMSS`, `YYYYMMDD_HHMMSS` and ShotRename's own
/// `YYYY-MM-DD_HHMMSS` output, so replanning a renamed file keeps the time
/// even when no metadata survives.
pub fn datetime_from_filename(path: &Path) -> Option<NaiveDateTime> {
    let stem = path.file_stem()?.to_str()?;
    let bytes = stem.as_bytes();

    for index in 0..bytes.len() {
        if index + 14 <= bytes.len() && bytes[index..index + 14].iter().all(u8::is_ascii_digit) {
            let value = &stem[index..index + 14];
            if let Some(parsed) = parse_compact(value) {
                return Some(parsed);
            }
        }
        if index + 17 <= bytes.len()
            && bytes[index..index + 8].iter().all(u8::is_ascii_digit)
            && bytes[index + 8] == b'_'
            && bytes[index + 9..index + 15].iter().all(u8::is_ascii_digit)
        {
            let parsed = parse_parts(&stem[index..index + 8], &stem[index + 9..index + 15]);
            if parsed.is_some() {
                return parsed;
            }
        }
        if index + 17 <= bytes.len()
            && bytes[index..index + 4].iter().all(u8::is_ascii_digit)
            && bytes[index + 4] == b'-'
            && bytes[index + 5..index + 7].iter().all(u8::is_ascii_digit)
            && bytes[index + 7] == b'-'
            && bytes[index + 8..index + 10].iter().all(u8::is_ascii_digit)
            && bytes[index + 10] == b'_'
            && bytes[index + 11..index + 17].iter().all(u8::is_ascii_digit)
        {
            let parsed = NaiveDate::parse_from_str(&stem[index..index + 10], "%Y-%m-%d")
                .ok()
                .and_then(|date| {
                    chrono::NaiveTime::parse_from_str(&stem[index + 11..index + 17], "%H%M%S")
                        .ok()
                        .map(|time| date.and_time(time))
                });
            if parsed.is_some() {
                return parsed;
            }
        }
    }
    None
}

fn parse_compact(value: &str) -> Option<NaiveDateTime> {
    parse_parts(&value[0..8], &value[8..14])
}

fn parse_parts(date: &str, time: &str) -> Option<NaiveDateTime> {
    let date = NaiveDate::parse_from_str(date, "%Y%m%d").ok()?;
    let time = chrono::NaiveTime::parse_from_str(time, "%H%M%S").ok()?;
    Some(date.and_time(time))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_dji_old_and_new_names() {
        assert_eq!(detect_family(&PathBuf::from("DJI_0001.MP4")), Family::Dji);
        assert_eq!(
            detect_family(&PathBuf::from("DJI_20241121143015_0001_D.MP4")),
            Family::Dji
        );
    }

    #[test]
    fn detects_gopro_chapters() {
        assert_eq!(detect_family(&PathBuf::from("GOPR0001.MP4")), Family::GoPro);
        assert_eq!(detect_family(&PathBuf::from("GP010001.MP4")), Family::GoPro);
        assert_eq!(detect_family(&PathBuf::from("GX010001.MP4")), Family::GoPro);
    }

    #[test]
    fn detects_insta360_insv() {
        assert_eq!(
            detect_family(&PathBuf::from("VID_20241121_143015_00_001.insv")),
            Family::Insta360
        );
    }

    #[test]
    fn parses_dji_timestamp() {
        let value =
            datetime_from_filename(&PathBuf::from("DJI_20241121143015_0001_D.MP4")).unwrap();
        assert_eq!(value.to_string(), "2024-11-21 14:30:15");
    }

    #[test]
    fn parses_insta360_timestamp() {
        let value =
            datetime_from_filename(&PathBuf::from("VID_20241121_143015_00_001.insv")).unwrap();
        assert_eq!(value.to_string(), "2024-11-21 14:30:15");
    }

    #[test]
    fn parses_renamed_output_timestamp() {
        let value =
            datetime_from_filename(&PathBuf::from("2024-11-21_143015_Insta360_00.insv")).unwrap();
        assert_eq!(value.to_string(), "2024-11-21 14:30:15");
    }

    #[test]
    fn filters_temporary_and_excluded() {
        assert!(is_temporary(&PathBuf::from("clip.MP4.part")));
        assert!(!is_temporary(&PathBuf::from("clip.MP4")));
        assert!(is_excluded_dir(".git"));
        assert!(is_excluded_dir("node_modules"));
        assert!(!is_excluded_dir("footage"));
    }
}
