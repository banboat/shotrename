use crate::template::SanitizeMode;

/// Maximum number of characters kept in one path component.
pub const MAX_COMPONENT_CHARS: usize = 120;
/// Maximum number of bytes kept in one path component.
pub const MAX_COMPONENT_BYTES: usize = 240;

/// Normalise a rendered name into a safe single path component.
///
/// Rules, matching the behaviour we documented for RenamerX but with
/// Unicode kept by default:
/// - letters and digits are kept (`unicode` mode keeps any alphabetic char);
/// - `_` and `-` are kept and act as separators;
/// - whitespace becomes the template separator;
/// - every other character is dropped;
/// - leading, trailing and repeated separators are collapsed.
pub fn sanitize(input: &str, separator: char, mode: SanitizeMode) -> String {
    let separator = match separator {
        '_' | '-' => separator,
        _ => '_',
    };
    let mut out = String::with_capacity(input.len());
    let mut last_was_separator = true;

    for ch in input.chars() {
        let keep = match mode {
            SanitizeMode::Unicode => ch.is_alphanumeric(),
            SanitizeMode::Ascii => ch.is_ascii_alphanumeric(),
        };
        if keep {
            out.push(ch);
            last_was_separator = false;
            continue;
        }
        if ch == '_' || ch == '-' {
            if !last_was_separator {
                out.push(ch);
                last_was_separator = true;
            }
            continue;
        }
        if ch.is_whitespace() && !last_was_separator {
            out.push(separator);
            last_was_separator = true;
        }
        // Anything else is dropped on purpose.
    }

    while out.chars().last().is_some_and(is_separator) {
        out.pop();
    }

    truncate(&out, MAX_COMPONENT_CHARS, MAX_COMPONENT_BYTES)
}

fn is_separator(ch: char) -> bool {
    ch == '_' || ch == '-'
}

fn truncate(input: &str, max_chars: usize, max_bytes: usize) -> String {
    if input.chars().count() <= max_chars && input.len() <= max_bytes {
        return input.to_string();
    }
    let mut out = String::new();
    for ch in input.chars() {
        if out.chars().count() >= max_chars || out.len() + ch.len_utf8() > max_bytes {
            break;
        }
        out.push(ch);
    }
    while out.chars().last().is_some_and(is_separator) {
        out.pop();
    }
    out
}

/// Return a non-empty fallback when a rendered name sanitises to nothing.
pub fn sanitize_or_renamed(input: &str, separator: char, mode: SanitizeMode) -> String {
    let value = sanitize(input, separator, mode);
    if value.is_empty() {
        "renamed".to_string()
    } else {
        value
    }
}

/// Sanitise every component of a relative directory template result.
pub fn sanitize_relative_dir(input: &str, separator: char, mode: SanitizeMode) -> Vec<String> {
    input
        .split(['/', '\\'])
        .map(|part| sanitize(part, separator, mode))
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unicode_and_collapses_separators() {
        assert_eq!(
            sanitize(
                "2026-11-21__DJI  Osmo Action (5)",
                '_',
                SanitizeMode::Unicode
            ),
            "2026-11-21_DJI_Osmo_Action_5"
        );
    }

    #[test]
    fn trims_leading_and_trailing_separators() {
        assert_eq!(
            sanitize("_-_hello world_-_", '-', SanitizeMode::Unicode),
            "hello-world"
        );
    }

    #[test]
    fn keeps_chinese_in_unicode_mode() {
        assert_eq!(
            sanitize("上海 黄浦 航拍", '_', SanitizeMode::Unicode),
            "上海_黄浦_航拍"
        );
    }

    #[test]
    fn ascii_mode_drops_chinese() {
        assert_eq!(
            sanitize("上海 Huangpu", '_', SanitizeMode::Ascii),
            "Huangpu"
        );
    }

    #[test]
    fn drops_other_punctuation() {
        assert_eq!(
            sanitize("a/b\\c:d*e?f\"g<h>i|j", '_', SanitizeMode::Unicode),
            "abcdefghij"
        );
    }

    #[test]
    fn empty_input_falls_back() {
        assert_eq!(
            sanitize_or_renamed("///", '_', SanitizeMode::Unicode),
            "renamed"
        );
    }

    #[test]
    fn truncates_to_char_limit() {
        let input = "a".repeat(200);
        assert_eq!(
            sanitize(&input, '_', SanitizeMode::Unicode).chars().count(),
            120
        );
    }

    #[test]
    fn directory_is_split_and_sanitised() {
        assert_eq!(
            sanitize_relative_dir("2026/11 月//DJI", '_', SanitizeMode::Unicode),
            vec!["2026", "11_月", "DJI"]
        );
    }
}
