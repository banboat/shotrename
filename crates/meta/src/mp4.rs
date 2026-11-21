use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Seconds between the QuickTime epoch (1904-01-01) and the Unix epoch.
const QUICKTIME_EPOCH_OFFSET: i64 = 2_082_844_800;

#[derive(Debug, Clone, Default)]
pub struct Mp4Metadata {
    pub major_brand: Option<String>,
    pub creation_time_utc: Option<DateTime<Utc>>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub gps: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    kind: [u8; 4],
    header_len: u64,
    size: u64,
}

impl BoxHeader {
    fn payload_start(&self, box_start: u64) -> u64 {
        box_start + self.header_len
    }

    fn payload_len(&self) -> u64 {
        self.size.saturating_sub(self.header_len)
    }
}

pub fn read_metadata(path: &Path) -> Result<Mp4Metadata> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let file_len = file.metadata()?.len();
    let mut metadata = Mp4Metadata::default();

    let mut offset = 0u64;
    while offset + 8 <= file_len {
        file.seek(SeekFrom::Start(offset))?;
        let Some(header) = read_header(&mut file, file_len, offset)? else {
            break;
        };
        match &header.kind {
            b"ftyp" => {
                if header.payload_len() >= 4 {
                    let mut brand = [0u8; 4];
                    file.read_exact(&mut brand)?;
                    metadata.major_brand = Some(String::from_utf8_lossy(&brand).trim().to_string());
                }
            }
            b"moov" => {
                read_moov(
                    &mut file,
                    header.payload_start(offset),
                    header.payload_len(),
                    &mut metadata,
                )?;
                break;
            }
            _ => {}
        }
        offset = offset
            .checked_add(header.size)
            .context("mp4 box offset overflow")?;
        if header.size == 0 {
            break;
        }
    }
    Ok(metadata)
}

fn read_moov(file: &mut File, start: u64, len: u64, metadata: &mut Mp4Metadata) -> Result<()> {
    let end = start.saturating_add(len);
    let mut offset = start;
    while offset + 8 <= end {
        file.seek(SeekFrom::Start(offset))?;
        let Some(header) = read_header(file, end, offset)? else {
            break;
        };
        match &header.kind {
            b"mvhd" => read_mvhd(file, header, metadata)?,
            b"udta" => read_udta(
                file,
                header.payload_start(offset),
                header.payload_len(),
                metadata,
            )?,
            _ => {}
        }
        offset = offset
            .checked_add(header.size)
            .context("moov box offset overflow")?;
        if header.size == 0 {
            break;
        }
    }
    Ok(())
}

fn read_mvhd(file: &mut File, header: BoxHeader, metadata: &mut Mp4Metadata) -> Result<()> {
    if header.payload_len() < 4 {
        return Ok(());
    }
    let mut version = [0u8; 4];
    file.read_exact(&mut version)?;
    let creation = if version[0] == 1 {
        let mut raw = [0u8; 8];
        if header.payload_len() < 12 {
            return Ok(());
        }
        file.read_exact(&mut raw)?;
        u64::from_be_bytes(raw)
    } else {
        let mut raw = [0u8; 4];
        if header.payload_len() < 8 {
            return Ok(());
        }
        file.read_exact(&mut raw)?;
        u32::from_be_bytes(raw) as u64
    };
    if creation == 0 {
        return Ok(());
    }
    let unix = creation as i64 - QUICKTIME_EPOCH_OFFSET;
    if let Some(utc) = DateTime::<Utc>::from_timestamp(unix, 0) {
        metadata.creation_time_utc = Some(utc);
    }
    Ok(())
}

fn read_udta(file: &mut File, start: u64, len: u64, metadata: &mut Mp4Metadata) -> Result<()> {
    let end = start.saturating_add(len);
    let mut offset = start;
    while offset + 8 <= end {
        file.seek(SeekFrom::Start(offset))?;
        let Some(header) = read_header(file, end, offset)? else {
            break;
        };
        let value = match &header.kind {
            b"\xa9mak" | b"\xa9mod" | b"\xa9swr" | b"\xa9day" | b"\xa9xyz" => {
                read_data_box(file, header.payload_start(offset), header.payload_len())?
            }
            _ => None,
        };
        if let Some(value) = value {
            match &header.kind {
                b"\xa9mak" => metadata.make = Some(value),
                b"\xa9mod" => metadata.model = Some(value),
                b"\xa9swr" => metadata.software = Some(value),
                b"\xa9xyz" => metadata.gps = parse_iso6709(&value),
                _ => {}
            }
        }
        offset = offset
            .checked_add(header.size)
            .context("udta box offset overflow")?;
        if header.size == 0 {
            break;
        }
    }
    Ok(())
}

fn read_data_box(file: &mut File, start: u64, len: u64) -> Result<Option<String>> {
    let end = start.saturating_add(len);
    file.seek(SeekFrom::Start(start))?;
    let Some(header) = read_header(file, end, start)? else {
        return Ok(None);
    };
    if &header.kind != b"data" {
        return Ok(None);
    }
    let payload_len = header.payload_len().min(4096);
    let mut buffer = vec![0u8; payload_len as usize];
    file.read_exact(&mut buffer)?;
    // data box payload: 4 byte type indicator + 4 byte locale + text.
    if buffer.len() <= 8 {
        return Ok(None);
    }
    let text = &buffer[8..];
    let text = text.split(|byte| *byte == 0).next().unwrap_or_default();
    let value = String::from_utf8_lossy(text).trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn parse_iso6709(value: &str) -> Option<(f64, f64)> {
    // +31.2304+121.4737/ or +31.2304+121.4737+012.3/
    let trimmed = value.trim_end_matches('/');
    let mut splits = Vec::new();
    for (index, ch) in trimmed.char_indices() {
        if index > 0 && (ch == '+' || ch == '-') {
            splits.push(index);
        }
    }
    let first = *splits.first()?;
    let lat = trimmed[..first].parse::<f64>().ok()?;
    let lon_end = splits.get(1).copied().unwrap_or(trimmed.len());
    let lon = trimmed[first..lon_end].parse::<f64>().ok()?;
    Some((lat, lon))
}

fn read_header(file: &mut File, end: u64, offset: u64) -> Result<Option<BoxHeader>> {
    let mut raw = [0u8; 8];
    if file.read_exact(&mut raw).is_err() {
        return Ok(None);
    }
    let mut size = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as u64;
    let kind = [raw[4], raw[5], raw[6], raw[7]];
    let mut header_len = 8u64;
    if size == 1 {
        let mut large = [0u8; 8];
        file.read_exact(&mut large)?;
        size = u64::from_be_bytes(large);
        header_len = 16;
    } else if size == 0 {
        size = end.saturating_sub(offset);
    }
    if size < header_len {
        return Ok(None);
    }
    Ok(Some(BoxHeader {
        kind,
        header_len,
        size,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn data_box(text: &str) -> Vec<u8> {
        let mut payload = vec![0u8; 8];
        payload.extend_from_slice(text.as_bytes());
        atom(b"data", &payload)
    }

    fn fixture() -> Vec<u8> {
        let mut mvhd_payload = vec![0u8; 4]; // version 0 + flags
        let creation: u32 = (1_732_210_215 + QUICKTIME_EPOCH_OFFSET) as u32;
        mvhd_payload.extend_from_slice(&creation.to_be_bytes());
        mvhd_payload.extend_from_slice(&[0u8; 96]);
        let mvhd = atom(b"mvhd", &mvhd_payload);

        let mut udta_payload = Vec::new();
        udta_payload.extend_from_slice(&atom(b"\xa9mak", &data_box("DJI")));
        udta_payload.extend_from_slice(&atom(b"\xa9mod", &data_box("Osmo Action 5 Pro")));
        udta_payload.extend_from_slice(&atom(b"\xa9xyz", &data_box("+31.2304+121.4737/")));
        let udta = atom(b"udta", &udta_payload);

        let mut moov_payload = Vec::new();
        moov_payload.extend_from_slice(&mvhd);
        moov_payload.extend_from_slice(&udta);
        let moov = atom(b"moov", &moov_payload);

        let mut ftyp_payload = Vec::new();
        ftyp_payload.extend_from_slice(b"qt  ");
        ftyp_payload.extend_from_slice(&[0u8; 4]);
        ftyp_payload.extend_from_slice(b"qt  ");
        let ftyp = atom(b"ftyp", &ftyp_payload);

        [ftyp, moov].concat()
    }

    #[test]
    fn parses_creation_make_model_gps() {
        let path: PathBuf = std::env::temp_dir().join(format!(
            "shotrename-mp4-{}-{}.mp4",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, fixture()).unwrap();
        let metadata = read_metadata(&path).unwrap();
        assert_eq!(metadata.make.as_deref(), Some("DJI"));
        assert_eq!(metadata.model.as_deref(), Some("Osmo Action 5 Pro"));
        assert!(metadata.creation_time_utc.is_some());
        assert_eq!(metadata.gps.unwrap().0, 31.2304);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn tolerates_non_mp4_files() {
        let path =
            std::env::temp_dir().join(format!("shotrename-mp4-bad-{}.mp4", std::process::id()));
        fs::write(&path, b"not an mp4").unwrap();
        assert!(read_metadata(&path).unwrap().creation_time_utc.is_none());
        let _ = fs::remove_file(path);
    }
}
