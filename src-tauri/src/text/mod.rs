//! Text handling for the viewer and editor: binary sniffing, encoding detection, windowed
//! decoding, line endings and lossless round trips (plan §2.3–§2.4).
//!
//! Everything here is pure and works on bytes already in memory; nothing touches the disk.
//! The policy (docs/adr/0013-text-round-trip.md):
//!
//! - **Binary** when the first [`SNIFF_LEN`] bytes contain a NUL (UTF-16 files with a BOM are
//!   exempt: their ASCII characters contain NULs).
//! - **Encoding:** a BOM (UTF-8, UTF-16LE, UTF-16BE) wins; otherwise valid UTF-8 (a sequence cut
//!   by the end of a partial window still counts as valid); otherwise `windows-1252`, which
//!   maps every byte and so always decodes.
//! - **Windows** never split a multi-byte sequence or a `\r\n` pair: those bytes are held back
//!   and `consumed` says where the next window starts.
//! - **Editable** only when complete and `encode(decode(bytes)) == bytes` (with line endings
//!   normalised and restored, unless they are mixed, in which case saving normalises them to
//!   the dominant ending). UTF-16 is read-only: `encoding_rs` cannot encode it.

pub mod lines;
pub mod terminal;

use encoding_rs::{EncoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

use crate::domain::file::{LineEnding, ReadOnlyReason};
use crate::error::{AppError, AppResult};

/// Bytes inspected for NUL bytes when deciding whether a file is binary.
pub const SNIFF_LEN: usize = 8 * 1024;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Whether the start of a file looks binary (a NUL byte in the first [`SNIFF_LEN`] bytes).
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_LEN)].contains(&0)
}

/// The encoding of a file and the length of its byte order mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub encoding: &'static Encoding,
    pub bom_len: usize,
}

/// Detects the encoding of the start of a file. `complete` says whether `bytes` is the whole
/// file: a partial window may end in the middle of a UTF-8 sequence and still be UTF-8.
pub fn detect_encoding(bytes: &[u8], complete: bool) -> Detected {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        return Detected { encoding, bom_len };
    }
    let utf8 = match std::str::from_utf8(bytes) {
        Ok(_) => true,
        // `error_len() == None`: the input ends inside a sequence that could still be valid.
        Err(error) => !complete && error.error_len().is_none(),
    };
    Detected {
        encoding: if utf8 { UTF_8 } else { WINDOWS_1252 },
        bom_len: 0,
    }
}

/// Resolves one of the encoding labels this module produces (`OpenedFile::encoding`).
pub fn encoding_for_label(label: &str) -> AppResult<&'static Encoding> {
    match label {
        "utf-8" => Ok(UTF_8),
        "windows-1252" => Ok(WINDOWS_1252),
        "utf-16le" => Ok(UTF_16LE),
        "utf-16be" => Ok(UTF_16BE),
        other => Err(AppError::validation(format!(
            "unsupported encoding: {other}"
        ))),
    }
}

/// The lowercase WHATWG label used in the IPC contract.
pub fn label(encoding: &'static Encoding) -> &'static str {
    if encoding == UTF_8 {
        "utf-8"
    } else if encoding == UTF_16LE {
        "utf-16le"
    } else if encoding == UTF_16BE {
        "utf-16be"
    } else {
        "windows-1252"
    }
}

fn is_utf16(encoding: &'static Encoding) -> bool {
    encoding == UTF_16LE || encoding == UTF_16BE
}

/// A decoded window of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Text exactly as decoded (original line endings).
    pub raw: String,
    /// Bytes of the input used; the rest was held back for the next window.
    pub consumed: usize,
    /// Malformed input was replaced with U+FFFD.
    pub had_errors: bool,
}

/// Decodes `bytes` (no BOM) with `encoding`. Unless `complete`, a trailing incomplete
/// multi-byte sequence and a trailing `\r` are held back so the next window starts with them.
pub fn decode_window(encoding: &'static Encoding, bytes: &[u8], complete: bool) -> Decoded {
    let held = if complete {
        0
    } else {
        incomplete_tail(encoding, bytes)
    };
    let mut consumed = bytes.len() - held;
    let (text, had_errors) = encoding.decode_without_bom_handling(&bytes[..consumed]);
    let mut raw = text.into_owned();
    if !complete && raw.ends_with('\r') {
        raw.pop();
        consumed -= if is_utf16(encoding) { 2 } else { 1 };
    }
    Decoded {
        raw,
        consumed,
        had_errors,
    }
}

/// Length of an incomplete code point at the end of `bytes`.
fn incomplete_tail(encoding: &'static Encoding, bytes: &[u8]) -> usize {
    let n = bytes.len();
    if encoding == UTF_8 {
        for back in 1..=n.min(3) {
            let byte = bytes[n - back];
            if byte & 0xC0 == 0x80 {
                continue; // continuation byte: keep looking for the lead byte
            }
            if byte >= 0xC0 {
                let needed = if byte >= 0xF0 {
                    4
                } else if byte >= 0xE0 {
                    3
                } else {
                    2
                };
                return if back < needed { back } else { 0 };
            }
            return 0; // ASCII
        }
        0
    } else if is_utf16(encoding) {
        let odd = n % 2;
        let even = n - odd;
        if even >= 2 {
            let pair = [bytes[even - 2], bytes[even - 1]];
            let unit = if encoding == UTF_16LE {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            };
            if (0xD800..=0xDBFF).contains(&unit) {
                return odd + 2; // a high surrogate waiting for its pair
            }
        }
        odd
    } else {
        0
    }
}

/// The first window of a file, ready for the viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedText {
    /// Normalised to `\n`; empty for binary files.
    pub text: String,
    /// Bytes used, including the BOM.
    pub consumed: usize,
    pub binary: bool,
    pub encoding: &'static str,
    pub bom: bool,
    pub line_ending: LineEnding,
    pub mixed_line_endings: bool,
    pub read_only: Option<ReadOnlyReason>,
}

/// Interprets the start of a file (`complete` when `bytes` is all of it).
pub fn open(bytes: &[u8], complete: bool) -> OpenedText {
    let detected = detect_encoding(bytes, complete);
    if !is_utf16(detected.encoding) && looks_binary(bytes) {
        return OpenedText {
            text: String::new(),
            consumed: bytes.len(),
            binary: true,
            encoding: label(detected.encoding),
            bom: false,
            line_ending: LineEnding::Lf,
            mixed_line_endings: false,
            read_only: Some(ReadOnlyReason::Binary),
        };
    }
    let body = &bytes[detected.bom_len..];
    let decoded = decode_window(detected.encoding, body, complete);
    let stats = lines::scan(&decoded.raw);
    let line_ending = stats.dominant();
    let mixed = stats.mixed();
    let text = lines::normalise(&decoded.raw);
    let bom = detected.bom_len > 0;
    let encoding = label(detected.encoding);

    let read_only = if is_utf16(detected.encoding) {
        Some(ReadOnlyReason::UnsupportedEncoding)
    } else if !complete {
        Some(ReadOnlyReason::Partial)
    } else if decoded.had_errors {
        Some(ReadOnlyReason::Encoding)
    } else {
        let round_trip = if mixed {
            // Saving will normalise the endings; what must survive is everything else.
            encode_text(&decoded.raw, detected.encoding)
                .ok()
                .map(|encoded| with_bom(encoded, bom))
        } else {
            encode_for_save(&text, encoding, bom, line_ending).ok()
        };
        (round_trip.as_deref() != Some(bytes)).then_some(ReadOnlyReason::Encoding)
    };

    OpenedText {
        text,
        consumed: detected.bom_len + decoded.consumed,
        binary: false,
        encoding,
        bom,
        line_ending,
        mixed_line_endings: mixed,
        read_only,
    }
}

fn with_bom(encoded: Vec<u8>, bom: bool) -> Vec<u8> {
    if !bom {
        return encoded;
    }
    let mut out = Vec::with_capacity(UTF8_BOM.len() + encoded.len());
    out.extend_from_slice(UTF8_BOM);
    out.extend(encoded);
    out
}

/// Encodes editor text (`\n` line endings) for saving: UTF-8 (optionally with a BOM) or
/// windows-1252, with `\n` converted to `line_ending`. A character windows-1252 cannot
/// represent is a `Validation` error naming it and its line (never an HTML entity).
pub fn encode_for_save(
    text: &str,
    encoding_label: &str,
    bom: bool,
    line_ending: LineEnding,
) -> AppResult<Vec<u8>> {
    let encoding = encoding_for_label(encoding_label)?;
    if is_utf16(encoding) {
        return Err(AppError::validation(
            "UTF-16 files cannot be saved; Gem can only show them",
        ));
    }
    if bom && encoding != UTF_8 {
        return Err(AppError::validation(
            "only UTF-8 files can have a byte order mark",
        ));
    }
    let encoded = encode_text(text, encoding)?;
    Ok(with_bom(
        lines::denormalise_bytes(encoded, line_ending),
        bom,
    ))
}

/// Encodes `text` as is (UTF-8 or windows-1252), refusing unmappable characters.
fn encode_text(text: &str, encoding: &'static Encoding) -> AppResult<Vec<u8>> {
    if encoding == UTF_8 {
        return Ok(text.as_bytes().to_vec());
    }
    let mut encoder = encoding.new_encoder();
    let mut out = Vec::with_capacity(text.len() + 16);
    let mut read_total = 0;
    loop {
        let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(
            &text[read_total..],
            &mut out,
            true,
        );
        read_total += read;
        match result {
            EncoderResult::InputEmpty => return Ok(out),
            EncoderResult::OutputFull => out.reserve(text.len() - read_total + 16),
            EncoderResult::Unmappable(c) => {
                let at = read_total - c.len_utf8();
                let line = text[..at].matches('\n').count() + 1;
                return Err(AppError::validation(format!(
                    "The character “{c}” (U+{:04X}) on line {line} cannot be saved as {}. \
                     Remove it, or convert the file to UTF-8 first.",
                    u32::from(c),
                    label(encoding)
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens a whole file and checks that saving it unchanged gives back the same bytes.
    fn assert_round_trip(bytes: &[u8]) -> OpenedText {
        let opened = open(bytes, true);
        assert_eq!(opened.read_only, None, "{bytes:?} should be editable");
        let saved = encode_for_save(
            &opened.text,
            opened.encoding,
            opened.bom,
            opened.line_ending,
        )
        .unwrap();
        assert_eq!(saved, bytes);
        opened
    }

    /// Decodes `bytes` window by window (as "Load more" does) and joins the normalised text.
    fn decode_in_windows(bytes: &[u8], window: usize) -> String {
        let first_len = window.min(bytes.len());
        let first = open(&bytes[..first_len], first_len == bytes.len());
        let encoding = encoding_for_label(first.encoding).unwrap();
        let mut text = first.text;
        let mut offset = first.consumed;
        while offset < bytes.len() {
            let end = (offset + window).min(bytes.len());
            let chunk = decode_window(encoding, &bytes[offset..end], end == bytes.len());
            assert!(chunk.consumed > 0 || end == bytes.len(), "no progress");
            text.push_str(&lines::normalise(&chunk.raw));
            offset += chunk.consumed;
        }
        text
    }

    #[test]
    fn utf8_with_and_without_bom() {
        let plain = assert_round_trip("héllo wörld\n".as_bytes());
        assert_eq!((plain.encoding, plain.bom), ("utf-8", false));
        let bom = assert_round_trip(b"\xEF\xBB\xBFname: caf\xC3\xA9\n");
        assert_eq!((bom.encoding, bom.bom), ("utf-8", true));
        assert_eq!(bom.text, "name: café\n");
        assert_eq!(bom.consumed, 15);
    }

    #[test]
    fn latin1_accents_fall_back_to_windows_1252() {
        let bytes = b"Ol\xE1, ma\xE7\xE3 \x80\r\nfim\r\n";
        let opened = assert_round_trip(bytes);
        assert_eq!(opened.encoding, "windows-1252");
        assert_eq!(opened.text, "Olá, maçã €\nfim\n");
        assert_eq!(opened.line_ending, LineEnding::Crlf);
    }

    #[test]
    fn every_byte_round_trips_in_windows_1252() {
        // Includes 0x81, 0x8D, 0x8F, 0x90 and 0x9D, which windows-1252 leaves undefined.
        let bytes: Vec<u8> = (1u8..=255).filter(|b| *b != b'\r').collect();
        let opened = assert_round_trip(&bytes);
        assert_eq!(opened.encoding, "windows-1252");
    }

    #[test]
    fn line_endings_round_trip() {
        assert_eq!(
            assert_round_trip(b"a\r\nb\r\n").line_ending,
            LineEnding::Crlf
        );
        assert_eq!(assert_round_trip(b"a\rb\r").line_ending, LineEnding::Cr);
        assert_eq!(assert_round_trip(b"a\nb\n").line_ending, LineEnding::Lf);
    }

    #[test]
    fn final_newline_presence_is_preserved() {
        let without = assert_round_trip(b"last line");
        assert_eq!(without.text, "last line");
        let with = assert_round_trip(b"last line\r\n");
        assert_eq!(with.text, "last line\n");
    }

    #[test]
    fn empty_file_is_editable_utf8() {
        let opened = assert_round_trip(b"");
        assert_eq!(opened.encoding, "utf-8");
        assert_eq!(opened.text, "");
        assert_eq!(opened.line_ending, LineEnding::Lf);
        assert!(!opened.binary);
    }

    #[test]
    fn mixed_line_endings_are_editable_and_save_the_dominant_one() {
        let bytes = b"a\r\nb\r\nc\nd";
        let opened = open(bytes, true);
        assert!(opened.mixed_line_endings);
        assert_eq!(opened.read_only, None);
        assert_eq!(opened.line_ending, LineEnding::Crlf);
        assert_eq!(opened.text, "a\nb\nc\nd");
        let saved = encode_for_save(
            &opened.text,
            opened.encoding,
            opened.bom,
            opened.line_ending,
        )
        .unwrap();
        assert_eq!(saved, b"a\r\nb\r\nc\r\nd");
    }

    #[test]
    fn invalid_utf8_after_a_bom_is_read_only() {
        let opened = open(b"\xEF\xBB\xBFok \xFF\xFE bad", true);
        assert_eq!(opened.encoding, "utf-8");
        assert_eq!(opened.read_only, Some(ReadOnlyReason::Encoding));
    }

    #[test]
    fn utf16_is_shown_but_read_only() {
        let mut le = vec![0xFF, 0xFE];
        for unit in "hi\r\nyo".encode_utf16() {
            le.extend(unit.to_le_bytes());
        }
        let opened = open(&le, true);
        assert_eq!(opened.encoding, "utf-16le");
        assert!(!opened.binary);
        assert_eq!(opened.text, "hi\nyo");
        assert_eq!(opened.line_ending, LineEnding::Crlf);
        assert_eq!(opened.read_only, Some(ReadOnlyReason::UnsupportedEncoding));

        let mut be = vec![0xFE, 0xFF];
        for unit in "ok".encode_utf16() {
            be.extend(unit.to_be_bytes());
        }
        let opened = open(&be, true);
        assert_eq!(opened.encoding, "utf-16be");
        assert_eq!(opened.text, "ok");
        assert!(encode_for_save("ok", "utf-16be", false, LineEnding::Lf).is_err());
    }

    #[test]
    fn nul_bytes_mean_binary() {
        let opened = open(b"\x89PNG\r\n\x1a\n\0\0\0", true);
        assert!(opened.binary);
        assert_eq!(opened.text, "");
        assert_eq!(opened.read_only, Some(ReadOnlyReason::Binary));
        // Only the sniff window counts.
        let mut late = vec![b'a'; SNIFF_LEN];
        late.push(0);
        assert!(!looks_binary(&late));
    }

    #[test]
    fn partial_windows_are_read_only_and_utf8_survives_a_cut_sequence() {
        let bytes = "abc€".as_bytes(); // € is 3 bytes
        let cut = &bytes[..bytes.len() - 1];
        let opened = open(cut, false);
        assert_eq!(opened.encoding, "utf-8");
        assert_eq!(opened.text, "abc");
        assert_eq!(opened.consumed, 3);
        assert_eq!(opened.read_only, Some(ReadOnlyReason::Partial));
        // The same cut in a complete file is not UTF-8.
        assert_eq!(open(cut, true).encoding, "windows-1252");
    }

    #[test]
    fn emoji_split_across_windows() {
        let text = "start 😀 middle 😀😀 end\n".repeat(20);
        // Windows of at least 4 bytes, the longest UTF-8 sequence (the editor reads 64 KiB+).
        for window in [4, 5, 7, 64, 101] {
            assert_eq!(decode_in_windows(text.as_bytes(), window), text, "{window}");
        }
    }

    #[test]
    fn crlf_split_across_windows() {
        let text = "line one\r\nline two\r\nthree\r\n";
        let bytes = text.as_bytes();
        // Window ends right after the first `\r`.
        let first = open(&bytes[..9], false);
        assert_eq!(first.text, "line one");
        assert_eq!(first.consumed, 8, "the \\r is held back");
        for window in [2, 3, 9, 10, 11] {
            assert_eq!(
                decode_in_windows(bytes, window),
                "line one\nline two\nthree\n"
            );
        }
        // A lone CR file decoded in windows of 1 byte still makes progress at the end.
        assert_eq!(decode_in_windows(b"a\rb\r", 4), "a\nb\n");
    }

    #[test]
    fn utf16_windows_hold_back_odd_bytes_and_surrogates() {
        let mut le = vec![0xFF, 0xFE];
        for unit in "a😀\r\nb".encode_utf16() {
            le.extend(unit.to_le_bytes());
        }
        for window in [4, 5, 6, 7] {
            assert_eq!(decode_in_windows(&le, window), "a😀\nb", "{window}");
        }
    }

    #[test]
    fn windows_1252_refuses_unmappable_characters_with_their_line() {
        let error = encode_for_save(
            "ok\nstill ok\ncheck ✓ here",
            "windows-1252",
            false,
            LineEnding::Lf,
        )
        .unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::Validation);
        let message = error.to_string();
        assert!(message.contains('✓'), "{message}");
        assert!(message.contains("U+2713"), "{message}");
        assert!(message.contains("line 3"), "{message}");
        assert!(!message.contains("&#"), "{message}");
    }

    #[test]
    fn save_validates_the_format() {
        assert!(encode_for_save("x", "windows-1252", true, LineEnding::Lf).is_err());
        assert!(encode_for_save("x", "koi8-r", false, LineEnding::Lf).is_err());
        assert_eq!(
            encode_for_save("x\n", "utf-8", true, LineEnding::Crlf).unwrap(),
            b"\xEF\xBB\xBFx\r\n"
        );
    }

    #[test]
    fn edits_keep_the_rest_byte_for_byte() {
        // Latin-1 CRLF file: change one line, every other byte stays the same.
        let original = b"caf\xE9\r\nna\xEFve\r\nend\r\n";
        let opened = open(original, true);
        let edited = opened.text.replace("naïve", "naive");
        let saved =
            encode_for_save(&edited, opened.encoding, opened.bom, opened.line_ending).unwrap();
        assert_eq!(saved, b"caf\xE9\r\nnaive\r\nend\r\n");
    }
}
