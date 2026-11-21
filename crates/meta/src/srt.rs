use chrono::NaiveDateTime;

#[derive(Debug, Clone, Default)]
pub struct SrtTelemetry {
    pub captured: Option<NaiveDateTime>,
    pub gps: Option<(f64, f64)>,
    pub altitude: Option<f64>,
    pub relative_altitude: Option<f64>,
}

/// Parse the first timestamp and telemetry values from a DJI SRT file.
///
/// DJI writes lines such as:
/// `2024-11-21 14:30:15.000 [latitude: 31.2304] [longitude: 121.4737] ...`
pub fn parse(text: &str) -> SrtTelemetry {
    let mut telemetry = SrtTelemetry::default();
    for line in text.lines().take(80) {
        if telemetry.captured.is_none() {
            if let Some(value) = find_datetime(line) {
                telemetry.captured = Some(value);
            }
        }
        if telemetry.gps.is_none() {
            let lat = bracket_value(line, "latitude");
            let lon = bracket_value(line, "longitude");
            if let (Some(lat), Some(lon)) = (lat, lon) {
                telemetry.gps = Some((lat, lon));
            }
        }
        if telemetry.altitude.is_none() {
            telemetry.altitude = bracket_value(line, "altitude");
        }
        if telemetry.relative_altitude.is_none() {
            telemetry.relative_altitude = bracket_value(line, "rel_alt");
        }
        if telemetry.captured.is_some() && telemetry.gps.is_some() && telemetry.altitude.is_some() {
            break;
        }
    }
    telemetry
}

fn find_datetime(line: &str) -> Option<NaiveDateTime> {
    let bytes = line.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    for index in 0..=bytes.len() - 19 {
        let candidate = &line[index..index + 19];
        if NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S").is_ok() {
            return NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S").ok();
        }
    }
    None
}

fn bracket_value(line: &str, key: &str) -> Option<f64> {
    let start = line.find(&format!("[{key}: "))? + key.len() + 3;
    let rest = &line[start..];
    let end = rest.find(']')?;
    let value = rest[..end].split_whitespace().next().unwrap_or_default();
    value.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dji_telemetry() {
        let text = "\
1
00:00:00,000 --> 00:00:00,033
<font size=\"28\">SrtCnt : 1, DiffTime : 33ms
2024-11-21 14:30:15.000
[iso : 100] [shutter : 1/60] [fnum : 2.8] [latitude: 31.2304] [longitude: 121.4737] [rel_alt: 12.300 abs_alt: 20.100] [altitude: 20.100]</font>
";
        let value = parse(text);
        assert_eq!(value.captured.unwrap().to_string(), "2024-11-21 14:30:15");
        assert_eq!(value.gps.unwrap(), (31.2304, 121.4737));
        assert_eq!(value.altitude, Some(20.1));
        assert_eq!(value.relative_altitude, Some(12.3));
    }

    #[test]
    fn returns_empty_for_plain_srt() {
        let value = parse("1\n00:00:01,000 --> 00:00:02,000\nhello\n");
        assert!(value.captured.is_none());
        assert!(value.gps.is_none());
    }
}
