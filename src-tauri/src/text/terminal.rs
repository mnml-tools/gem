//! Turning the raw output of a remote command into text for the viewer (ADR 0014).
//!
//! One [`OutputDecoder`] per stream (stdout, stderr), fed chunk by chunk as they arrive:
//!
//! - **UTF-8, lossy**, without ever splitting a character: an incomplete sequence at the end of
//!   a chunk waits for the next one; invalid bytes become U+FFFD.
//! - **ANSI escape sequences are removed** (CSI `ESC [ … final`, OSC/DCS/APC/PM/SOS strings up to
//!   BEL or `ESC \`, charset designations and other two-character `ESC x` sequences, and the
//!   8-bit CSI), even when a sequence is split across chunks. Other control characters are
//!   dropped; `\n` and `\t` are kept.
//! - **Carriage returns:** `\r\n` is a line break. A lone `\r` redraws the line (progress bars):
//!   such a line is held back until it ends and only its last state is shown.
//!
//! [`OutputDecoder::push`] returns completed lines; [`OutputDecoder::take_partial`] returns the
//! start of a line that is still being written (so `echo -n` output shows up), unless that line
//! is being redrawn.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Escape {
    #[default]
    None,
    /// After `ESC`.
    Start,
    /// After `ESC` + an intermediate (`(`, `)`, `#`, …): one more character ends it.
    Argument,
    /// Inside `ESC [` (or 8-bit CSI) until a final byte `@`..`~`.
    Csi,
    /// Inside an OSC/DCS/SOS/PM/APC string until BEL or ST.
    Text,
    /// `ESC` inside such a string (`ESC \` ends it).
    StringEnd,
}

#[derive(Debug, Default)]
pub struct OutputDecoder {
    /// An incomplete UTF-8 sequence from the end of the last chunk.
    carry: Vec<u8>,
    escape: Escape,
    /// Text of the current line not shown yet.
    line: String,
    /// A `\r` was seen; what follows decides between a line break and a redraw.
    carriage_return: bool,
    /// The current line was redrawn at least once: hold it until it ends.
    redrawing: bool,
    /// Part of the current line was already shown by `take_partial`.
    shown_partial: bool,
    /// Completed text waiting to be returned.
    ready: String,
}

impl OutputDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk; returns the text of the lines it completed.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut buffer = std::mem::take(&mut self.carry);
        buffer.extend_from_slice(bytes);
        let mut rest: &[u8] = &buffer;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.feed(text);
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    self.feed(std::str::from_utf8(valid).unwrap_or_default());
                    match error.error_len() {
                        Some(invalid) => {
                            self.feed("\u{FFFD}");
                            rest = &after[invalid..];
                        }
                        None => {
                            self.carry = after.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        std::mem::take(&mut self.ready)
    }

    /// The start of the current line, if it is not being redrawn (shown at batch flushes).
    pub fn take_partial(&mut self) -> String {
        let mut text = std::mem::take(&mut self.ready);
        if !self.redrawing && !self.line.is_empty() {
            self.shown_partial = true;
            text.push_str(&std::mem::take(&mut self.line));
        }
        text
    }

    /// Ends the stream: everything left, including an unfinished last line.
    pub fn finish(&mut self) -> String {
        if !self.carry.is_empty() {
            let carry = std::mem::take(&mut self.carry);
            self.feed(&String::from_utf8_lossy(&carry));
        }
        let mut text = std::mem::take(&mut self.ready);
        text.push_str(&std::mem::take(&mut self.line));
        *self = Self::default();
        text
    }

    fn feed(&mut self, text: &str) {
        for c in text.chars() {
            self.char(c);
        }
    }

    fn char(&mut self, c: char) {
        match self.escape {
            Escape::Start => {
                self.escape = match c {
                    '[' => Escape::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => Escape::Text,
                    '(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%' | ' ' => Escape::Argument,
                    _ => Escape::None,
                };
                return;
            }
            Escape::Argument => {
                self.escape = Escape::None;
                return;
            }
            Escape::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    self.escape = Escape::None;
                }
                return;
            }
            Escape::Text => {
                match c {
                    '\u{07}' | '\u{9c}' => self.escape = Escape::None,
                    '\u{1b}' => self.escape = Escape::StringEnd,
                    _ => {}
                }
                return;
            }
            Escape::StringEnd => {
                self.escape = match c {
                    '\\' => Escape::None,
                    '\u{1b}' => Escape::StringEnd,
                    _ => Escape::Text,
                };
                return;
            }
            Escape::None => {}
        }
        match c {
            '\u{1b}' => self.escape = Escape::Start,
            '\u{9b}' => self.escape = Escape::Csi,
            '\r' => self.carriage_return = true,
            '\n' => {
                self.carriage_return = false;
                self.ready.push_str(&self.line);
                self.ready.push('\n');
                self.line.clear();
                self.redrawing = false;
                self.shown_partial = false;
            }
            c if c == '\t' || !c.is_control() => {
                if std::mem::take(&mut self.carriage_return) {
                    self.redraw();
                }
                self.line.push(c);
            }
            _ => {} // other control characters (BEL, backspace, …)
        }
    }

    /// A lone `\r` followed by text: the line starts over.
    fn redraw(&mut self) {
        if std::mem::take(&mut self.shown_partial) {
            // What was shown cannot be taken back: end it and redraw on a new line.
            self.ready.push('\n');
        }
        self.line.clear();
        self.redrawing = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&[u8]]) -> String {
        let mut decoder = OutputDecoder::new();
        let mut text = String::new();
        for chunk in chunks {
            text.push_str(&decoder.push(chunk));
        }
        text.push_str(&decoder.finish());
        text
    }

    /// Feeds `bytes` one byte at a time and in a few chunk sizes; all must agree.
    fn decode_split(bytes: &[u8]) -> String {
        let whole = decode(&[bytes]);
        for size in [1, 2, 3, 5, 7] {
            let chunks: Vec<&[u8]> = bytes.chunks(size).collect();
            assert_eq!(decode(&chunks), whole, "chunks of {size}");
        }
        whole
    }

    #[test]
    fn utf8_is_never_split_and_invalid_bytes_are_replaced() {
        let text = "olá 日本語 🎉\n".repeat(3);
        assert_eq!(decode_split(text.as_bytes()), text);
        assert_eq!(
            decode(&[b"bad \xff\xfe byte\n"]),
            "bad \u{FFFD}\u{FFFD} byte\n"
        );
        // A sequence cut by the end of the output is replaced, not lost.
        assert_eq!(decode(&[b"cut \xe6\x97"]), "cut \u{FFFD}");
    }

    #[test]
    fn ansi_sequences_are_stripped_across_chunks() {
        let colored = b"\x1b[1;31merror\x1b[0m: \x1b]0;title\x07x\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\ \x1b(Bdone\x1b7\x1b8\n";
        assert_eq!(decode_split(colored), "error: xlink done\n");
        assert_eq!(
            decode_split("\u{9b}32mgreen\u{9b}0m\n".as_bytes()),
            "green\n"
        );
        assert_eq!(decode_split(b"a\x07b\x08c\td\n"), "abc\td\n");
    }

    #[test]
    fn line_endings_and_progress_redraws() {
        assert_eq!(decode_split(b"one\r\ntwo\r\n"), "one\ntwo\n");
        assert_eq!(
            decode_split(b"pulling 10%\rpulling 55%\r\x1b[Kpulling 100%\ndone\n"),
            "pulling 100%\ndone\n"
        );
        assert_eq!(
            decode_split(b"no newline at the end"),
            "no newline at the end"
        );
        assert_eq!(
            decode_split(b"ends with a carriage return\r"),
            "ends with a carriage return"
        );
    }

    #[test]
    fn partial_lines_show_up_unless_redrawn() {
        let mut decoder = OutputDecoder::new();
        assert_eq!(decoder.push(b"waiting"), "");
        assert_eq!(decoder.take_partial(), "waiting");
        assert_eq!(decoder.push(b"... ok\n"), "... ok\n");
        // A progress line is held back, then only its last state is shown.
        assert_eq!(decoder.push(b"10%\r20%\r30%"), "");
        assert_eq!(decoder.take_partial(), "");
        assert_eq!(decoder.push(b"\n"), "30%\n");
        // Shown, then redrawn: the redraw starts on a new line.
        decoder.push(b"step 1");
        assert_eq!(decoder.take_partial(), "step 1");
        assert_eq!(decoder.push(b"\rstep 2\n"), "\nstep 2\n");
        assert_eq!(decoder.finish(), "");
    }
}
