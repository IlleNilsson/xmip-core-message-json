//! The scan that says a byte sequence is one JSON document, and picks the
//! top-level `"type"` or `"$type"` out of it on the way past.
//!
//! No tree, no decoded numbers, no allocation beyond the one string the
//! shape asks for. The scan stops at the first byte that cannot continue a
//! document and hands that byte back with the reason. The cursor — peek,
//! whitespace, a string — is the Foundation's [`Scan`] (ADR-0044); the
//! grammar of a document is this file's.

use message::Stop;
use message::scan::Scan;

/// Deeper than this and the document is refused rather than the stack.
const DEEPEST: usize = 512;

/// Scan `bytes` as one document; the announced type when it has one.
///
/// # Errors
/// The reason and the byte at which the bytes stopped being a document.
pub fn document(bytes: &[u8]) -> Result<Option<String>, Stop> {
    let mut scan = Scan::new(bytes);
    scan.whitespace();
    let announced = value(&mut scan, 0)?;
    scan.whitespace();
    if scan.at < bytes.len() {
        return Err(("content after the document", scan.at));
    }
    Ok(announced)
}

/// Past the value under the cursor; the type it announces, which only the
/// top-level object can.
fn value(scan: &mut Scan<'_>, depth: usize) -> Result<Option<String>, Stop> {
    if depth > DEEPEST {
        return Err(("nested too deep", scan.at));
    }
    match scan.peek() {
        Some(b'{') => object(scan, depth),
        Some(b'[') => array(scan, depth).map(|()| None),
        Some(b'"') => scan.string().map(|_| None),
        Some(b't') => literal(scan, b"true").map(|()| None),
        Some(b'f') => literal(scan, b"false").map(|()| None),
        Some(b'n') => literal(scan, b"null").map(|()| None),
        Some(b'-' | b'0'..=b'9') => number(scan).map(|()| None),
        _ => Err(("expected a value", scan.at)),
    }
}

/// Past the object under the cursor. At depth zero, the `"type"` it
/// announces, else its `"$type"`; a nested object announces nothing.
fn object(scan: &mut Scan<'_>, depth: usize) -> Result<Option<String>, Stop> {
    scan.at += 1;
    scan.whitespace();
    if scan.peek() == Some(b'}') {
        scan.at += 1;
        return Ok(None);
    }
    let mut announced = None;
    let mut dollar = None;
    loop {
        if scan.peek() != Some(b'"') {
            return Err(("expected a key", scan.at));
        }
        let key = scan.string()?;
        scan.whitespace();
        if scan.peek() != Some(b':') {
            return Err(("expected a colon", scan.at));
        }
        scan.at += 1;
        scan.whitespace();
        let top_level_type = depth == 0 && (key == b"type" || key == b"$type");
        if top_level_type && scan.peek() == Some(b'"') {
            let decoded = decode(scan.string()?);
            if key == b"type" {
                announced = Some(decoded);
            } else {
                dollar = Some(decoded);
            }
        } else {
            value(scan, depth + 1)?;
        }
        scan.whitespace();
        match scan.peek() {
            Some(b',') => {
                scan.at += 1;
                scan.whitespace();
            }
            Some(b'}') => {
                scan.at += 1;
                return Ok(announced.or(dollar));
            }
            _ => return Err(("expected a comma or the end of the object", scan.at)),
        }
    }
}

fn array(scan: &mut Scan<'_>, depth: usize) -> Result<(), Stop> {
    scan.at += 1;
    scan.whitespace();
    if scan.peek() == Some(b']') {
        scan.at += 1;
        return Ok(());
    }
    loop {
        value(scan, depth + 1)?;
        scan.whitespace();
        match scan.peek() {
            Some(b',') => {
                scan.at += 1;
                scan.whitespace();
            }
            Some(b']') => {
                scan.at += 1;
                return Ok(());
            }
            _ => return Err(("expected a comma or the end of the array", scan.at)),
        }
    }
}

fn literal(scan: &mut Scan<'_>, word: &[u8]) -> Result<(), Stop> {
    if scan.rest().starts_with(word) {
        scan.at += word.len();
        Ok(())
    } else {
        Err(("expected a value", scan.at))
    }
}

fn number(scan: &mut Scan<'_>) -> Result<(), Stop> {
    if scan.peek() == Some(b'-') {
        scan.at += 1;
    }
    if digits(scan) == 0 {
        return Err(("expected a digit", scan.at));
    }
    if scan.peek() == Some(b'.') {
        scan.at += 1;
        if digits(scan) == 0 {
            return Err(("expected a digit", scan.at));
        }
    }
    if matches!(scan.peek(), Some(b'e' | b'E')) {
        scan.at += 1;
        if matches!(scan.peek(), Some(b'+' | b'-')) {
            scan.at += 1;
        }
        if digits(scan) == 0 {
            return Err(("expected a digit", scan.at));
        }
    }
    Ok(())
}

/// Past the digits under the cursor; how many there were.
fn digits(scan: &mut Scan<'_>) -> usize {
    let start = scan.at;
    while matches!(scan.peek(), Some(b'0'..=b'9')) {
        scan.at += 1;
    }
    scan.at - start
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
        assert_eq!(document(br#"[{"type":"inner"}]"#).expect("scans"), None);
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
