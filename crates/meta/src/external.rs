use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct FfprobeInfo {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub duration_secs: Option<f64>,
    pub codec: Option<String>,
    pub creation_time_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct ExifInfo {
    pub model: Option<String>,
    pub lens: Option<String>,
    pub create_date: Option<NaiveDateTime>,
    pub gps: Option<(f64, f64)>,
    pub duration_secs: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
}

fn run(binary: &str, args: &[&str], path: &Path) -> Option<String> {
    let output = Command::new(binary).args(args).arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

pub fn ffprobe(path: &Path) -> Option<FfprobeInfo> {
    let stdout = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ],
        path,
    )?;
    let value: Value = serde_json::from_str(&stdout).ok()?;
    let mut info = FfprobeInfo::default();

    if let Some(duration) = value
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<f64>().ok())
    {
        info.duration_secs = Some(duration);
    }
    if let Some(created) = value
        .get("format")
        .and_then(|format| format.get("tags"))
        .and_then(|tags| tags.get("creation_time"))
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    {
        info.creation_time_utc = Some(created.with_timezone(&Utc));
    }

    if let Some(streams) = value.get("streams").and_then(Value::as_array) {
        if let Some(stream) = streams
            .iter()
            .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
        {
            info.codec = stream
                .get("codec_name")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            info.width = stream
                .get("width")
                .and_then(Value::as_u64)
                .map(|value| value as u32);
            info.height = stream
                .get("height")
                .and_then(Value::as_u64)
                .map(|value| value as u32);
            info.fps = stream
                .get("r_frame_rate")
                .and_then(Value::as_str)
                .and_then(parse_rate);
        }
    }
    Some(info)
}

pub fn exiftool(path: &Path) -> Option<ExifInfo> {
    let stdout = run(
        "exiftool",
        &[
            "-j",
            "-s",
            "-n",
            "-Model",
            "-LensModel",
            "-CreateDate",
            "-GPSLatitude",
            "-GPSLongitude",
            "-Duration",
            "-ImageWidth",
            "-ImageHeight",
            "-VideoFrameRate",
        ],
        path,
    )?;
    let value: Value = serde_json::from_str(&stdout).ok()?;
    let object = value.as_array()?.first()?.as_object()?;
    let lat = number_field(object, "GPSLatitude");
    let lon = number_field(object, "GPSLongitude");
    Some(ExifInfo {
        model: string_field(object, "Model"),
        lens: string_field(object, "LensModel"),
        create_date: object
            .get("CreateDate")
            .and_then(Value::as_str)
            .and_then(|value| NaiveDateTime::parse_from_str(value, "%Y:%m:%d %H:%M:%S").ok()),
        gps: lat.zip(lon),
        duration_secs: number_field(object, "Duration"),
        width: number_field(object, "ImageWidth").map(|value| value as u32),
        height: number_field(object, "ImageHeight").map(|value| value as u32),
        fps: number_field(object, "VideoFrameRate"),
    })
}

fn string_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn number_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key).and_then(|value| match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        _ => None,
    })
}

fn parse_rate(value: &str) -> Option<f64> {
    match value.split_once('/') {
        Some((num, den)) => {
            let num = num.parse::<f64>().ok()?;
            let den = den.parse::<f64>().ok()?;
            if den == 0.0 {
                None
            } else {
                Some(num / den)
            }
        }
        None => value.parse::<f64>().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fractional_frame_rate() {
        assert_eq!(parse_rate("30000/1001").unwrap().round(), 30.0);
        assert_eq!(parse_rate("60").unwrap(), 60.0);
        assert!(parse_rate("1/0").is_none());
    }
}
