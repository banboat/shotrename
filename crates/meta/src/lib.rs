pub mod detect;
pub mod external;
pub mod group;
pub mod mp4;
pub mod places;
pub mod srt;

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime};
use serde::Serialize;
use shotrename_core::{FieldValues, Template};
use std::fs;
use std::path::{Path, PathBuf};

pub use detect::{
    datetime_from_filename, detect_family, extension, is_excluded_dir, is_supported_video,
    is_temporary, Family, RELATED_EXTENSIONS, TEMP_EXTENSIONS, VIDEO_EXTENSIONS,
};
pub use places::Place;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeSource {
    DjiSrt,
    ExifTool,
    QuickTime,
    Filename,
    FileMtime,
}

impl TimeSource {
    pub fn label(self) -> &'static str {
        match self {
            TimeSource::DjiSrt => "dji-srt",
            TimeSource::ExifTool => "exiftool",
            TimeSource::QuickTime => "quicktime",
            TimeSource::Filename => "filename",
            TimeSource::FileMtime => "file-mtime",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaProbe {
    pub path: PathBuf,
    pub size: u64,
    pub family: Family,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub captured: Option<NaiveDateTime>,
    pub time_source: TimeSource,
    pub gps: Option<(f64, f64)>,
    pub place: Option<Place>,
    pub altitude: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub duration_secs: Option<f64>,
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProbeOptions {
    /// UTC offset in seconds. Defaults to the current system offset.
    pub tz_offset_secs: Option<i32>,
}

impl MediaProbe {
    pub fn field_values(&self, template: &Template) -> FieldValues {
        let mut values = std::collections::BTreeMap::new();
        let place_name = self
            .place
            .as_ref()
            .map(|place| place.name.clone())
            .unwrap_or_default();
        values.insert("camera".into(), self.camera.clone().unwrap_or_default());
        values.insert("lens".into(), self.lens.clone().unwrap_or_default());
        values.insert("location".into(), place_name.clone());
        values.insert(
            "location_zh".into(),
            self.place
                .as_ref()
                .and_then(|place| place.name_zh.clone())
                .unwrap_or(place_name),
        );
        values.insert(
            "country".into(),
            self.place
                .as_ref()
                .map(|place| place.country.clone())
                .unwrap_or_default(),
        );
        values.insert(
            "gps".into(),
            self.gps
                .map(|(lat, lon)| format!("{lat:.6},{lon:.6}"))
                .unwrap_or_default(),
        );
        values.insert("resolution".into(), self.resolution());
        values.insert("fps".into(), self.fps());
        values.insert("duration".into(), self.duration());
        values.insert("codec".into(), self.codec.clone().unwrap_or_default());
        values.insert(
            "original".into(),
            self.path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string(),
        );
        values.insert("family".into(), self.family.label().to_string());
        values.insert(
            "width".into(),
            self.width
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );
        values.insert(
            "height".into(),
            self.height
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );
        values.insert(
            "altitude".into(),
            self.altitude
                .map(|value| format!("{value:.1}"))
                .unwrap_or_default(),
        );
        values.insert("seq".into(), String::new());

        FieldValues {
            captured: self.captured,
            date_format: template.format.date.clone(),
            time_format: template.format.time.clone(),
            values,
        }
    }

    pub fn resolution(&self) -> String {
        match (self.width, self.height) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => String::new(),
        }
    }

    pub fn fps(&self) -> String {
        let Some(fps) = self.fps else {
            return String::new();
        };
        let rounded = (fps * 100.0).round() / 100.0;
        let mut text = format!("{rounded:.2}");
        while text.ends_with('0') {
            text.pop();
        }
        while text.ends_with('.') {
            text.pop();
        }
        text
    }

    pub fn duration(&self) -> String {
        self.duration_secs
            .map(|seconds| format!("{}", seconds.round() as u64))
            .unwrap_or_default()
    }
}

pub fn probe(path: &Path, options: &ProbeOptions) -> Result<MediaProbe> {
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let size = metadata.len();
    let mtime = metadata
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let offset = options
        .tz_offset_secs
        .unwrap_or_else(|| Local::now().offset().local_minus_utc());
    let offset = FixedOffset::east_opt(offset).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());

    let family = detect_family(path);
    let mp4 = mp4::read_metadata(path).unwrap_or_default();
    let ffprobe = external::ffprobe(path);
    let exif = external::exiftool(path);
    let telemetry = find_srt(path).and_then(|srt_path| {
        let text = fs::read_to_string(&srt_path).ok()?;
        Some(srt::parse(&text))
    });

    let (captured, time_source) = if let Some(value) = telemetry.as_ref().and_then(|t| t.captured) {
        (Some(value), TimeSource::DjiSrt)
    } else if let Some(value) = exif.as_ref().and_then(|e| e.create_date) {
        (Some(value), TimeSource::ExifTool)
    } else if let Some(value) = mp4.creation_time_utc {
        (
            Some(value.with_timezone(&offset).naive_local()),
            TimeSource::QuickTime,
        )
    } else if let Some(value) = ffprobe.as_ref().and_then(|f| f.creation_time_utc) {
        (
            Some(value.with_timezone(&offset).naive_local()),
            TimeSource::QuickTime,
        )
    } else if let Some(value) = datetime_from_filename(path) {
        (Some(value), TimeSource::Filename)
    } else {
        let local: DateTime<Local> = mtime.into();
        (Some(local.naive_local()), TimeSource::FileMtime)
    };

    let camera = exif
        .as_ref()
        .and_then(|e| e.model.clone())
        .or_else(|| mp4.model.clone())
        .or_else(|| mp4.make.clone())
        .or_else(|| Some(family.label().to_string()));
    let lens = exif.as_ref().and_then(|e| e.lens.clone());
    let gps = telemetry
        .as_ref()
        .and_then(|t| t.gps)
        .or_else(|| exif.as_ref().and_then(|e| e.gps))
        .or(mp4.gps);
    let place = gps.and_then(|(lat, lon)| places::lookup(lat, lon));
    let width = ffprobe
        .as_ref()
        .and_then(|f| f.width)
        .or_else(|| exif.as_ref().and_then(|e| e.width));
    let height = ffprobe
        .as_ref()
        .and_then(|f| f.height)
        .or_else(|| exif.as_ref().and_then(|e| e.height));
    let fps = ffprobe
        .as_ref()
        .and_then(|f| f.fps)
        .or_else(|| exif.as_ref().and_then(|e| e.fps));
    let duration_secs = ffprobe
        .as_ref()
        .and_then(|f| f.duration_secs)
        .or_else(|| exif.as_ref().and_then(|e| e.duration_secs));
    let codec = ffprobe.as_ref().and_then(|f| f.codec.clone());

    Ok(MediaProbe {
        path: path.to_path_buf(),
        size,
        family,
        camera,
        lens,
        captured,
        time_source,
        gps,
        place,
        altitude: telemetry.as_ref().and_then(|t| t.altitude),
        width,
        height,
        fps,
        duration_secs,
        codec,
    })
}

/// Find the sidecar SRT that belongs to `path`: same directory, same stem,
/// any extension case.
pub fn find_srt(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    for entry in fs::read_dir(parent).ok()? {
        let entry = entry.ok()?;
        let candidate = entry.path();
        if extension(&candidate) != "srt" {
            continue;
        }
        if candidate.file_stem().and_then(|value| value.to_str()) == Some(stem) {
            return Some(candidate);
        }
    }
    None
}

/// Sidecar files that should follow the main video.
///
/// Covers exact-stem siblings (`.LRF`, `.LRV`, `.SRT`, `.WAV`, `.THM`) and
/// language-tagged subtitles such as `DJI_0001.en.srt`.
pub fn related_files(path: &Path) -> Vec<PathBuf> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{stem}.");
    let mut related = Vec::new();
    let Ok(entries) = fs::read_dir(parent) else {
        return related;
    };
    for entry in entries.flatten() {
        let candidate = entry.path();
        if candidate == path || !candidate.is_file() {
            continue;
        }
        let ext = extension(&candidate);
        if !RELATED_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let Some(candidate_stem) = candidate.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let exact = candidate_stem == stem;
        let language_srt = ext == "srt"
            && candidate_stem.starts_with(&prefix)
            && candidate_stem.len() > prefix.len();
        // Insta360 proxies are `LRV_<same stamp>.lrv` next to `VID_<...>.insv`.
        let proxy = matches!(ext.as_str(), "lrv" | "lrf")
            && strip_media_prefix(candidate_stem).eq_ignore_ascii_case(strip_media_prefix(stem));
        if exact || language_srt || proxy {
            related.push(candidate);
        }
    }
    related.sort();
    related
}

fn strip_media_prefix(value: &str) -> &str {
    for prefix in ["VID_", "LRV_"] {
        if value.len() > 4 && value[..4].eq_ignore_ascii_case(prefix) {
            return &value[4..];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "shotrename-meta-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn finds_exact_and_language_sidecars() {
        let dir = temp_dir("related");
        for name in [
            "DJI_0001.MP4",
            "DJI_0001.SRT",
            "DJI_0001.LRF",
            "DJI_0001.en.srt",
            "DJI_0001.WAV",
            "DJI_0002.SRT",
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let related = related_files(&dir.join("DJI_0001.MP4"));
        let names: Vec<String> = related
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "DJI_0001.LRF",
                "DJI_0001.SRT",
                "DJI_0001.WAV",
                "DJI_0001.en.srt"
            ]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finds_insta360_proxy() {
        let dir = temp_dir("insta-proxy");
        for name in [
            "VID_20241121_143015_00_001.insv",
            "LRV_20241121_143015_00_001.lrv",
            "LRV_20241121_143015_10_001.lrv",
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let related = related_files(&dir.join("VID_20241121_143015_00_001.insv"));
        let names: Vec<String> = related
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["LRV_20241121_143015_00_001.lrv"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finds_srt_case_insensitively() {
        let dir = temp_dir("srt");
        fs::write(dir.join("clip.MP4"), b"x").unwrap();
        fs::write(dir.join("clip.srt"), b"x").unwrap();
        assert!(find_srt(&dir.join("clip.MP4")).is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn formats_fps_without_trailing_zeroes() {
        let probe = MediaProbe {
            path: PathBuf::from("a.mp4"),
            size: 0,
            family: Family::Generic,
            camera: None,
            lens: None,
            captured: None,
            time_source: TimeSource::FileMtime,
            gps: None,
            place: None,
            altitude: None,
            width: Some(3840),
            height: Some(2160),
            fps: Some(60.0),
            duration_secs: Some(12.6),
            codec: None,
        };
        assert_eq!(probe.fps(), "60");
        assert_eq!(probe.resolution(), "3840x2160");
        assert_eq!(probe.duration(), "13");
    }
}
