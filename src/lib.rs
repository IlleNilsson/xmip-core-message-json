#![forbid(unsafe_code)]

//! JSON: one document, one part. The shape reads the document far enough to
//! know it is one — a value, balanced to its end, with nothing after it but
//! whitespace — and names the type the document announces in a top-level
//! `"type"` or `"$type"` key when it has one (ADR-0047).
//!
//! That reading is a scan, not a parse: no tree is built and no value is
//! decoded, because a shape sections and names and a contract validates.
//! What cannot be sectioned is refused with the byte where the scan stopped.

mod scan;

use message::{Part, Shape, ShapeError, Shaped};
use stream::Stream;

/// The JSON shape.
#[derive(Clone, Copy, Debug, Default)]
pub struct Json;

/// The first byte after leading whitespace, if any.
fn first_significant(bytes: &[u8]) -> Option<u8> {
    bytes
        .iter()
        .copied()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

impl Shape for Json {
    fn technology(&self) -> &'static str {
        "json"
    }

    fn media_types(&self) -> &'static [&'static str] {
        &["application/json", "text/json"]
    }

    fn recognises(&self, bytes: &[u8]) -> bool {
        matches!(first_significant(bytes), Some(b'{' | b'['))
    }

    fn shape(&self, stream: &Stream) -> Result<Shaped, ShapeError> {
        let message_type = scan::document(stream.bytes())
            .map_err(|(reason, at)| ShapeError::new("json", reason).at(at))?;
        let media = stream
            .media_type()
            .map_or_else(|| "application/json".to_string(), str::to_string);
        Ok(Shaped {
            parts: vec![Part::new(None, stream.bytes(), Some(media))],
            message_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcore::StreamId;

    fn stream(bytes: &[u8], media: Option<&str>) -> Stream {
        Stream::new(StreamId::new(1), bytes.to_vec(), media.map(str::to_string))
    }

    #[test]
    fn a_document_is_one_part_and_its_top_level_type_key_is_the_announced_type() {
        let text = br#" {"id": 7, "items": [{"type": "line"}], "type": "order", "n": -1.5e3} "#;
        let shaped = Json.shape(&stream(text, None)).expect("well-formed");
        assert_eq!(shaped.parts.len(), 1);
        assert_eq!(shaped.parts[0].bytes, text);
        assert_eq!(
            shaped.parts[0].media_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(shaped.message_type.as_deref(), Some("order"));

        let dollar = Json
            .shape(&stream(br#"{"$type":"Invoice","type":7}"#, None))
            .expect("well-formed");
        assert_eq!(dollar.message_type.as_deref(), Some("Invoice"));

        let array = Json
            .shape(&stream(
                b"[1, true, null, \"s\\n\\u00e9\"]",
                Some("text/json"),
            ))
            .expect("an array is a document");
        assert_eq!(array.message_type, None);
        assert_eq!(array.parts[0].media_type.as_deref(), Some("text/json"));
    }

    #[test]
    fn a_document_that_does_not_close_or_that_trails_garbage_is_refused_where_it_fails() {
        let unclosed = Json
            .shape(&stream(br#"{"a": [1, 2}"#, None))
            .expect_err("bracket mismatch");
        assert_eq!(unclosed.offset, Some(11));
        assert_eq!(
            unclosed.to_string(),
            "json: expected a comma or the end of the array at byte 11"
        );

        let trailing = Json
            .shape(&stream(b"{} x", None))
            .expect_err("trailing garbage");
        assert_eq!(trailing.offset, Some(3));
        assert_eq!(trailing.reason, "content after the document");

        let cut = Json
            .shape(&stream(br#"{"a": "unterminated"#, None))
            .expect_err("unterminated string");
        assert_eq!(cut.offset, Some(19));

        let empty = Json.shape(&stream(b"  ", None)).expect_err("no value");
        assert_eq!(empty.offset, Some(2));

        let bad_escape = Json
            .shape(&stream(br#"["\x"]"#, None))
            .expect_err("bad escape");
        assert_eq!(bad_escape.offset, Some(3));
    }

    #[test]
    fn the_shape_claims_json_and_recognises_a_leading_brace_or_bracket() {
        assert_eq!(Json.technology(), "json");
        assert_eq!(Json.media_types(), &["application/json", "text/json"]);
        assert!(Json.recognises(b"  \n{"));
        assert!(Json.recognises(b"[1]"));
        assert!(!Json.recognises(b"7"));
        assert!(!Json.recognises(b"<a/>"));
        assert!(!Json.recognises(b""));
    }

    #[test]
    fn choose_picks_json_by_media_type_and_by_look() {
        let shapes: [&dyn Shape; 1] = [&Json];
        let by_media = message::choose(&shapes, &stream(b"x", Some("application/json")));
        assert_eq!(by_media.map(Shape::technology), Some("json"));
        let by_look = message::choose(&shapes, &stream(b"{}", None));
        assert_eq!(by_look.map(Shape::technology), Some("json"));
        assert!(message::choose(&shapes, &stream(b"plain", None)).is_none());
    }
}
