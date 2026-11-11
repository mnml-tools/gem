//! Line-ending detection and normalisation.
//!
//! The editor always works with `\n`. Opening a file counts each kind of line ending, picks the
//! dominant one and normalises everything to `\n`; saving converts `\n` back to the dominant
//! ending. Normalising after a lone `\r` at the end of a window is the caller's job: windowed
//! decoding holds such a `\r` back so a `\r\n` pair is never split (see [`super::decode`]).

use crate::domain::file::LineEnding;

/// How many line endings of each kind a text contains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineStats {
    pub lf: usize,
    pub crlf: usize,
    pub cr: usize,
}

impl LineStats {
    /// The most frequent ending; ties prefer LF, then CRLF. A text without line breaks is LF.
    pub fn dominant(&self) -> LineEnding {
        let mut best = (LineEnding::Lf, self.lf);
        for candidate in [(LineEnding::Crlf, self.crlf), (LineEnding::Cr, self.cr)] {
            if candidate.1 > best.1 {
                best = candidate;
            }
        }
        best.0
    }

    /// More than one kind of line ending is present.
    pub fn mixed(&self) -> bool {
        [self.lf, self.crlf, self.cr]
            .iter()
            .filter(|&&n| n > 0)
            .count()
            > 1
    }
}

/// Counts the line endings of `text`.
pub fn scan(text: &str) -> LineStats {
    let mut stats = LineStats::default();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                stats.crlf += 1;
                i += 1;
            }
            b'\r' => stats.cr += 1,
            b'\n' => stats.lf += 1,
            _ => {}
        }
        i += 1;
    }
    stats
}

/// Converts every `\r\n` and lone `\r` to `\n`.
pub fn normalise(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_owned();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The bytes of a line ending.
pub fn ending_bytes(ending: LineEnding) -> &'static [u8] {
    match ending {
        LineEnding::Lf => b"\n",
        LineEnding::Crlf => b"\r\n",
        LineEnding::Cr => b"\r",
    }
}

/// Replaces every `\n` byte with `ending`. Valid on UTF-8 and on ASCII-compatible single-byte
/// encodings, where `0x0A` only ever means a line feed.
pub fn denormalise_bytes(bytes: Vec<u8>, ending: LineEnding) -> Vec<u8> {
    if ending == LineEnding::Lf {
        return bytes;
    }
    let replacement = ending_bytes(ending);
    let breaks = bytes.iter().filter(|&&b| b == b'\n').count();
    let mut out = Vec::with_capacity(bytes.len() + breaks * (replacement.len() - 1));
    for byte in bytes {
        if byte == b'\n' {
            out.extend_from_slice(replacement);
        } else {
            out.push(byte);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_each_kind() {
        let stats = scan("a\nb\r\nc\rd\r\n");
        assert_eq!(
            stats,
            LineStats {
                lf: 1,
                crlf: 2,
                cr: 1
            }
        );
        assert_eq!(stats.dominant(), LineEnding::Crlf);
        assert!(stats.mixed());
    }

    #[test]
    fn dominant_defaults_and_ties() {
        assert_eq!(scan("no breaks").dominant(), LineEnding::Lf);
        assert!(!scan("no breaks").mixed());
        assert_eq!(scan("a\nb\r\n").dominant(), LineEnding::Lf);
        assert_eq!(scan("a\r\nb\r").dominant(), LineEnding::Crlf);
        assert_eq!(scan("a\rb\r").dominant(), LineEnding::Cr);
        assert!(!scan("a\r\nb\r\n").mixed());
    }

    #[test]
    fn a_cr_before_crlf_counts_as_cr_then_crlf() {
        let stats = scan("a\r\r\nb");
        assert_eq!((stats.cr, stats.crlf), (1, 1));
        assert_eq!(normalise("a\r\r\nb"), "a\n\nb");
    }

    #[test]
    fn normalises_and_back() {
        assert_eq!(normalise("a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(normalise("plain"), "plain");
        assert_eq!(
            denormalise_bytes(b"a\nb\n".to_vec(), LineEnding::Crlf),
            b"a\r\nb\r\n"
        );
        assert_eq!(denormalise_bytes(b"a\nb".to_vec(), LineEnding::Cr), b"a\rb");
        assert_eq!(denormalise_bytes(b"a\nb".to_vec(), LineEnding::Lf), b"a\nb");
    }
}
