//! The scan that says a byte sequence is one JSON document, and picks the
//! top-level `"type"` or `"$type"` out of it on the way past.
//!
//! No tree, no decoded numbers, no allocation beyond the one string the
//! shape asks for. The scan stops at the first byte that cannot continue a
//! document and hands that byte back with the reason.

/// Where a scan stopped and why.
pub type Stop = (&'static str, usize);

/// Deeper than this and the document is refused rather than the stack.
const DEEPEST: usize = 512;

/// Scan `bytes` as one document; the announced type when it has one.
///
/// # Errors
/// The reason and the byte at which the bytes stopped being a document.
pub fn document(bytes: &[u8]) -> Result<Option<String>, Stop> {
    let mut scan = Scan {
        bytes,
        at: 0,
        announced: None,
        dollar: None,
    };
    scan.whitespace();
    scan.value(0)?;
    scan.whitespace();
    if scan.at < bytes.len() {
        return Err(("content after the document", scan.at));
    }
    Ok(scan.announced.or(scan.dollar))
}

struct Scan<'a> {
    bytes: &'a [u8],
    at: usize,
    announced: Option<String>,
    dollar: Option<String>,
}

impl<'a> Scan<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<(), Stop> {
        if depth > DEEPEST {
            return Err(("nested too deep", self.at));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(|_| ()),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(("expected a value", self.at)),
        }
    }

    fn object(&mut self, depth: usize) -> Result<(), Stop> {
        self.at += 1;
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(());
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(("expected a key", self.at));
            }
            let key = self.string()?;
            self.whitespace();
            if self.peek() != Some(b':') {
                return Err(("expected a colon", self.at));
            }
            self.at += 1;
            self.whitespace();
            let top_level_type = depth == 0 && (key == b"type" || key == b"$type");
            if top_level_type && self.peek() == Some(b'"') {
                let raw = self.string()?;
                let decoded = decode(raw);
                if key == b"type" {
                    self.announced = Some(decoded);
                } else {
                    self.dollar = Some(decoded);
                }
            } else {
                self.value(depth + 1)?;
            }
            self.whitespace();
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    self.whitespace();
                }
                Some(b'}') => {
                    self.at += 1;
                    return Ok(());
                }
                _ => return Err(("expected a comma or the end of the object", self.at)),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<(), Stop> {
        self.at += 1;
        self.whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(());
        }
        loop {
            self.value(depth + 1)?;
            self.whitespace();
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    self.whitespace();
                }
                Some(b']') => {
                    self.at += 1;
                    return Ok(());
                }
                _ => return Err(("expected a comma or the end of the array", self.at)),
            }
        }
    }

    /// A string; the raw bytes between the quotes, escapes as written.
    fn string(&mut self) -> Result<&'a [u8], Stop> {
        let start = self.at + 1;
        self.at = start;
        loop {
            match self.peek() {
                None => return Err(("unterminated string", self.at)),
                Some(b'"') => {
                    let raw = &self.bytes[start..self.at];
                    self.at += 1;
                    return Ok(raw);
                }
                Some(b'\\') => {
                    self.at += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.at += 1;
                        }
                        Some(b'u') => {
                            let hex = self.bytes.get(self.at + 1..self.at + 5);
                            match hex {
                                Some(hex) if hex.iter().all(u8::is_ascii_hexdigit) => {
                                    self.at += 5;
                                }
                                _ => return Err(("bad escape", self.at)),
                            }
                        }
                        _ => return Err(("bad escape", self.at)),
                    }
                }
                Some(byte) if byte < 0x20 => return Err(("control byte in a string", self.at)),
                Some(_) => self.at += 1,
            }
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), Stop> {
        if self.bytes[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(())
        } else {
            Err(("expected a value", self.at))
        }
    }

    fn number(&mut self) -> Result<(), Stop> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        if self.digits() == 0 {
            return Err(("expected a digit", self.at));
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return Err(("expected a digit", self.at));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(("expected a digit", self.at));
            }
        }
        Ok(())
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }
}

/// The text of a scanned string: the escapes the scan accepted, decoded.
fn decode(raw: &[u8]) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        if raw[at] != b'\\' {
            let run = raw[at..]
                .iter()
                .position(|b| *b == b'\\')
                .unwrap_or(raw.len() - at);
            text.push_str(&String::from_utf8_lossy(&raw[at..at + run]));
            at += run;
            continue;
        }
        let escaped = raw.get(at + 1).copied().unwrap_or(b'\\');
        at += 2;
        match escaped {
            b'b' => text.push('\u{8}'),
            b'f' => text.push('\u{c}'),
            b'n' => text.push('\n'),
            b'r' => text.push('\r'),
            b't' => text.push('\t'),
            b'u' => {
                let code = std::str::from_utf8(raw.get(at..at + 4).unwrap_or(b""))
                    .ok()
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .and_then(char::from_u32)
                    .unwrap_or('\u{fffd}');
                text.push(code);
                at += 4;
            }
            other => text.push(char::from(other)),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_kind_scans_and_the_type_is_the_top_level_one() {
        let text = br#"{"type":"a\u00e9\n","x":{"type":"inner"},"n":[1,-2.5e+3,true,null]}"#;
        assert_eq!(document(text).expect("scans").as_deref(), Some("aé\n"));
        assert_eq!(document(b"  \"s\"  ").expect("scans"), None);
        assert_eq!(document(b"[]").expect("scans"), None);
        assert_eq!(document(b"{}").expect("scans"), None);
        assert_eq!(document(b"-0.5").expect("scans"), None);
        assert_eq!(
            document(br#"{"$type":"D","type":"T"}"#)
                .expect("scans")
                .as_deref(),
            Some("T")
        );
    }

    #[test]
    fn the_first_byte_that_cannot_continue_is_the_offset() {
        assert_eq!(document(b"{\"a\" 1}"), Err(("expected a colon", 5)));
        assert_eq!(
            document(b"[1 2]"),
            Err(("expected a comma or the end of the array", 3))
        );
        assert_eq!(document(b"{1:2}"), Err(("expected a key", 1)));
        assert_eq!(document(b"tru"), Err(("expected a value", 0)));
        assert_eq!(document(b"1."), Err(("expected a digit", 2)));
        assert_eq!(document(b"\"a\nb\""), Err(("control byte in a string", 2)));
        assert_eq!(document(b"[\"\\u12G4\"]"), Err(("bad escape", 3)));
        let deep = vec![b'['; DEEPEST + 2];
        assert_eq!(document(&deep), Err(("nested too deep", DEEPEST + 1)));
    }
}
