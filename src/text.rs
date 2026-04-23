use std::process::Output;

use encoding_rs::{Encoding, IBM866, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1251};

pub fn decode_output(output: &Output) -> String {
    let stderr = decode_bytes(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = decode_bytes(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    "без текста ошибки".to_string()
}

pub fn decode_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return sanitize_decoded_text(text);
    }

    let mut candidates = vec![
        candidate(bytes, UTF_8),
        candidate(bytes, WINDOWS_1251),
        candidate(bytes, IBM866),
    ];

    if looks_like_utf16(bytes) {
        candidates.push(candidate(bytes, UTF_16LE));
        candidates.push(candidate(bytes, UTF_16BE));
    }

    candidates
        .into_iter()
        .max_by_key(|item| item.score)
        .map(|item| item.value)
        .unwrap_or_else(|| sanitize_decoded_text(&String::from_utf8_lossy(bytes)))
}

fn candidate(bytes: &[u8], encoding: &'static Encoding) -> DecodedCandidate {
    let (value, _, had_errors) = encoding.decode(bytes);
    let owned = sanitize_decoded_text(&value);
    DecodedCandidate {
        score: score_text(&owned, had_errors, encoding.name()),
        value: owned,
    }
}

fn looks_like_utf16(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return false;
    }

    let nul_count = bytes.iter().filter(|byte| **byte == 0).count();
    nul_count.saturating_mul(4) >= bytes.len()
}

fn score_text(value: &str, had_errors: bool, encoding_name: &str) -> i64 {
    let mut score = if had_errors { -600 } else { 400 };

    if encoding_name == "UTF-8" {
        score += 40;
    }

    for ch in value.chars() {
        score += match ch {
            '\u{FFFD}' => -120,
            '\n' | '\r' | '\t' => 2,
            '"' | '\'' | '.' | ',' | ':' | ';' | '-' | '_' | '/' | '\\' | '(' | ')' | '[' | ']'
            | '{' | '}' | '@' | '#' | '$' | '%' | '&' | '*' | '+' | '=' | '?' | '!' => 1,
            ' '..='~' => 2,
            '\u{0400}'..='\u{04FF}' => 4,
            _ if ch.is_whitespace() => 1,
            _ if ch.is_control() => -8,
            _ if ch.is_alphanumeric() => 3,
            _ => 0,
        };
    }

    for marker in ["Рѕ", "Рµ", "СЃ", "С‚", "вЂ", "â", "Ð", "Ñ"] {
        if value.contains(marker) {
            score -= 30;
        }
    }

    score
}

fn sanitize_decoded_text(value: &str) -> String {
    value
        .trim_matches('\u{feff}')
        .replace('\0', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

struct DecodedCandidate {
    score: i64,
    value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_bytes() {
        let value = decode_bytes("Ошибка подключения".as_bytes());
        assert_eq!(value, "Ошибка подключения");
    }

    #[test]
    fn decodes_windows_1251_bytes() {
        let bytes = [
            0xcd, 0xe5, 0x20, 0xf3, 0xe4, 0xe0, 0xeb, 0xee, 0xf1, 0xfc, 0x20, 0xee, 0xf2, 0xea,
            0xf0, 0xfb, 0xf2, 0xfc,
        ];
        let value = decode_bytes(&bytes);
        assert_eq!(value, "Не удалось открыть");
    }

    #[test]
    fn decodes_ibm866_bytes() {
        let bytes = [
            0x8d, 0xa5, 0x20, 0xe3, 0xa4, 0xa0, 0xab, 0xae, 0xe1, 0xec, 0x20, 0xae, 0xe2, 0xaa,
            0xe0, 0xeb, 0xe2, 0xec,
        ];
        let value = decode_bytes(&bytes);
        assert_eq!(value, "Не удалось открыть");
    }

    #[test]
    fn strips_utf8_bom_and_nuls() {
        let bytes = b"\xef\xbb\xbf\x00\x00Telegram error\x00";
        let value = decode_bytes(bytes);
        assert_eq!(value, "Telegram error");
    }
}
