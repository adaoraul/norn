//! Line framing: raw bytes to complete lines, split on CRLF.
//!
//! The IRCv3 tag section may be up to 8191 bytes, separate from the 512-byte
//! message core (rule 2), so the reader must NOT assume a 512-byte total. The
//! framer buffers up to [`MAX_LINE_LEN`] and frames on `\n` (tolerating a bare
//! `\n` as well as `\r\n`).

/// Maximum bytes buffered for a single line: the 8191-byte tag budget plus the
/// 512-byte core (rule 2). Longer lines are discarded rather than buffered
/// without bound.
pub const MAX_LINE_LEN: usize = 8191 + 512;

/// A stateful, sans-I/O line framer. Feed it bytes; it yields complete lines.
#[derive(Debug)]
pub struct LineFramer {
    buf: Vec<u8>,
    max: usize,
    /// True while discarding the remainder of an over-long line until its `\n`.
    discarding: bool,
}

impl Default for LineFramer {
    fn default() -> Self {
        LineFramer::with_max(MAX_LINE_LEN)
    }
}

impl LineFramer {
    /// A framer with the default [`MAX_LINE_LEN`] budget.
    pub fn new() -> Self {
        LineFramer::default()
    }

    /// A framer with a custom maximum line length.
    pub fn with_max(max: usize) -> Self {
        LineFramer {
            buf: Vec::new(),
            max,
            discarding: false,
        }
    }

    /// Push received bytes and return any complete lines, with their trailing
    /// CRLF (or bare LF) stripped. Empty lines are skipped. Non-UTF-8 bytes are
    /// replaced (the design assumes a modern UTF-8 stream).
    pub fn push(&mut self, data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        self.buf.extend_from_slice(data);

        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            // Drain through the newline.
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop(); // drop '\n'
            if line.last() == Some(&b'\r') {
                line.pop(); // drop '\r'
            }

            if self.discarding {
                // This newline ends the over-long line we were dropping.
                self.discarding = false;
                continue;
            }
            if line.is_empty() {
                continue;
            }
            if line.len() > self.max {
                // A complete but over-budget line: drop it rather than emit.
                continue;
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }

        // No newline in what remains: if it has already grown past the budget,
        // drop it and discard until the next newline.
        if self.buf.len() > self.max {
            self.buf.clear();
            self.discarding = true;
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_multiple_crlf_lines() {
        let mut f = LineFramer::new();
        let lines = f.push(b"PING :a\r\nPONG :b\r\n");
        assert_eq!(lines, vec!["PING :a".to_string(), "PONG :b".to_string()]);
    }

    #[test]
    fn reassembles_a_line_split_across_pushes() {
        let mut f = LineFramer::new();
        assert!(f.push(b"PRIV").is_empty());
        assert!(f.push(b"MSG #c :hi").is_empty());
        assert_eq!(f.push(b"\r\n"), vec!["PRIVMSG #c :hi".to_string()]);
    }

    #[test]
    fn tolerates_bare_lf() {
        let mut f = LineFramer::new();
        assert_eq!(f.push(b"NICK x\n"), vec!["NICK x".to_string()]);
    }

    #[test]
    fn large_tag_section_over_512_is_not_truncated() {
        // Tag section well over 512 bytes but under the 8191 budget (rule 2).
        let mut f = LineFramer::new();
        let big = "x".repeat(4000);
        let line = format!("@bigtag={big} :s PRIVMSG #c :hi\r\n");
        let out = f.push(line.as_bytes());
        assert_eq!(out.len(), 1);
        assert!(out[0].contains(&big));
        assert!(out[0].ends_with(":hi"));
    }

    #[test]
    fn over_long_line_is_discarded_but_recovery_continues() {
        let mut f = LineFramer::with_max(16);
        // A line far longer than the tiny cap, then a good line.
        let junk = "y".repeat(100);
        let mut input = junk.into_bytes();
        input.extend_from_slice(b"\r\nGOOD :line\r\n");
        let out = f.push(&input);
        assert_eq!(out, vec!["GOOD :line".to_string()]);
    }
}
