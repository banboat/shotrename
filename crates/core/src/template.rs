use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelatedMode {
    /// Do not touch sidecar files.
    Off,
    /// Rename sidecars that can be matched; failures are reported per file.
    Follow,
    /// Rename the video and all its sidecars as one unit; any failure blocks the entry.
    Strict,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SanitizeMode {
    /// Keep Unicode letters and digits.
    Unicode,
    /// Keep ASCII letters and digits only.
    Ascii,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatOptions {
    #[serde(default = "default_date")]
    pub date: String,
    #[serde(default = "default_time")]
    pub time: String,
    #[serde(default = "default_separator")]
    pub separator: char,
    #[serde(default = "default_sanitize")]
    pub sanitize: SanitizeMode,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            date: default_date(),
            time: default_time(),
            separator: default_separator(),
            sanitize: default_sanitize(),
        }
    }
}

fn default_date() -> String {
    "%Y-%m-%d".to_string()
}
fn default_time() -> String {
    "%H%M%S".to_string()
}
fn default_separator() -> char {
    '_'
}
fn default_sanitize() -> SanitizeMode {
    SanitizeMode::Unicode
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub filename: String,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default = "default_related")]
    pub related: RelatedMode,
    #[serde(default)]
    pub format: FormatOptions,
}

fn default_related() -> RelatedMode {
    RelatedMode::Strict
}

impl Template {
    pub fn from_toml(input: &str) -> Result<Self> {
        let template: Template = toml::from_str(input).context("parse template TOML")?;
        template.validate()?;
        Ok(template)
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("template name must not be empty");
        }
        if self.filename.trim().is_empty() {
            bail!("template filename must not be empty");
        }
        // Fail early on unknown fields instead of producing empty names at render time.
        let values = FieldValues::sample();
        self.render_filename(&values)?;
        if let Some(directory) = &self.directory {
            let rendered = self.render(directory, &values)?;
            if rendered.trim_matches('/').is_empty() {
                bail!("template directory must not render to empty");
            }
        }
        Ok(())
    }

    pub fn render_filename(&self, values: &FieldValues) -> Result<String> {
        self.render(&self.filename, values)
    }

    pub fn render_directory(&self, values: &FieldValues) -> Result<Option<String>> {
        self.directory
            .as_ref()
            .map(|directory| self.render(directory, values))
            .transpose()
    }

    fn render(&self, template: &str, values: &FieldValues) -> Result<String> {
        let mut out = String::with_capacity(template.len() + 32);
        let mut chars = template.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '{' => {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        out.push('{');
                        continue;
                    }
                    let mut spec = String::new();
                    let mut closed = false;
                    for next in chars.by_ref() {
                        if next == '}' {
                            closed = true;
                            break;
                        }
                        spec.push(next);
                    }
                    if !closed {
                        bail!("unclosed field in template: {{{spec}");
                    }
                    let (field, format) = match spec.split_once(':') {
                        Some((field, format)) => (field.trim(), Some(format)),
                        None => (spec.trim(), None),
                    };
                    if field.is_empty() {
                        bail!("empty field name in template");
                    }
                    let value = values.lookup(field, format)?;
                    out.push_str(&value);
                }
                '}' => {
                    if chars.peek() == Some(&'}') {
                        chars.next();
                    }
                    out.push('}');
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FieldValues {
    pub captured: Option<NaiveDateTime>,
    pub date_format: String,
    pub time_format: String,
    pub values: BTreeMap<String, String>,
}

impl FieldValues {
    pub fn lookup(&self, field: &str, format: Option<&str>) -> Result<String> {
        match field {
            "date" => {
                let Some(captured) = self.captured else {
                    return Ok(String::new());
                };
                let format = format.unwrap_or(&self.date_format);
                Ok(captured.format(format).to_string())
            }
            "time" => {
                let Some(captured) = self.captured else {
                    return Ok(String::new());
                };
                let format = format.unwrap_or(&self.time_format);
                Ok(captured.format(format).to_string())
            }
            _ => match self.values.get(field) {
                Some(value) => Ok(value.clone()),
                None => bail!("unknown template field: {field}"),
            },
        }
    }

    fn sample() -> Self {
        let captured = chrono::NaiveDate::from_ymd_opt(2026, 11, 21)
            .and_then(|date| date.and_hms_opt(14, 30, 15));
        let mut values = BTreeMap::new();
        for field in [
            "camera",
            "lens",
            "location",
            "resolution",
            "fps",
            "duration",
            "codec",
            "original",
            "family",
            "width",
            "height",
            "seq",
        ] {
            values.insert(field.to_string(), "sample".to_string());
        }
        Self {
            captured,
            date_format: default_date(),
            time_format: default_time(),
            values,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> FieldValues {
        let captured =
            chrono::NaiveDate::from_ymd_opt(2026, 11, 21).and_then(|d| d.and_hms_opt(14, 30, 15));
        let mut map = BTreeMap::new();
        map.insert("camera".to_string(), "DJI Osmo Action 5".to_string());
        map.insert("lens".to_string(), String::new());
        map.insert("resolution".to_string(), "3840x2160".to_string());
        FieldValues {
            captured,
            date_format: "%Y-%m-%d".to_string(),
            time_format: "%H%M%S".to_string(),
            values: map,
        }
    }

    #[test]
    fn renders_fields_and_explicit_formats() {
        let template = Template {
            name: "test".into(),
            filename: "{date}_{time}_{camera}".into(),
            directory: Some("{date:%Y}/{date:%m}".into()),
            related: RelatedMode::Strict,
            format: FormatOptions::default(),
        };
        assert_eq!(
            template.render_filename(&values()).unwrap(),
            "2026-11-21_143015_DJI Osmo Action 5"
        );
        assert_eq!(
            template.render_directory(&values()).unwrap().as_deref(),
            Some("2026/11")
        );
    }

    #[test]
    fn unknown_field_is_an_error() {
        let template = Template {
            name: "test".into(),
            filename: "{nope}".into(),
            directory: None,
            related: RelatedMode::Off,
            format: FormatOptions::default(),
        };
        assert!(template.render_filename(&values()).is_err());
    }

    #[test]
    fn empty_value_renders_empty() {
        let template = Template {
            name: "test".into(),
            filename: "{date}_{lens}_{camera}".into(),
            directory: None,
            related: RelatedMode::Off,
            format: FormatOptions::default(),
        };
        assert_eq!(
            template.render_filename(&values()).unwrap(),
            "2026-11-21__DJI Osmo Action 5"
        );
    }

    #[test]
    fn parses_template_toml() {
        let template = Template::from_toml(
            r#"
name = "dji"
filename = "{date}_{time}_{camera}"
related = "strict"

[format]
date = "%Y-%m-%d"
time = "%H%M%S"
separator = "_"
sanitize = "unicode"
"#,
        )
        .unwrap();
        assert_eq!(template.name, "dji");
        assert_eq!(template.related, RelatedMode::Strict);
    }
}
