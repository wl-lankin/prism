//! The page's reports: schema, caps and cleaning. The page is untrusted (a script on
//! music.youtube.com can call `ytm_report` itself with anything), so a report counts only
//! when it has exactly the bridge's shape, and every field is cleaned on the way in.

use std::fmt;
use std::io::Write;

use reqwest::Url;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::covers::artwork_allowed;
use crate::radio::model::is_format_char;

/// A report larger than this, serialized, is refused whole.
pub const MAX_REPORT_BYTES: usize = 16 * 1024;
pub const MAX_TEXT_CHARS: usize = 256;
pub const MAX_ARTWORK_BYTES: usize = 2048;
/// A position may run this far past the duration (the page rounds both on its own).
pub const POSITION_SLACK_MS: u64 = 5_000;
/// Positions and durations are clamped to 100 hours.
pub const MAX_MS: u64 = 100 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    State(PageState),
    /// The track ended; a state report follows.
    Ended,
}

/// One state report, cleaned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageState {
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// An https URL on an artwork host; `None` for anything else.
    pub artwork: Option<String>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub can_next: bool,
    pub can_prev: bool,
    /// The player bar, its buttons and the video were all found.
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    TooLarge,
    Version,
    Shape(String),
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReportError::TooLarge => write!(f, "report larger than {MAX_REPORT_BYTES} bytes"),
            ReportError::Version => f.write_str("unknown report version"),
            ReportError::Shape(why) => write!(f, "malformed report: {why}"),
        }
    }
}

impl std::error::Error for ReportError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawState {
    playing: bool,
    title: String,
    artist: String,
    album: String,
    artwork: String,
    position_ms: f64,
    duration_ms: f64,
    can_next: bool,
    can_prev: bool,
    available: bool,
}

/// Counts bytes and fails once more than [`MAX_REPORT_BYTES`] were written, so an oversized
/// report is never serialized in full.
struct Capped(usize);

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        if self.0 > MAX_REPORT_BYTES {
            return Err(std::io::Error::other("too large"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn parse(value: Value) -> Result<Report, ReportError> {
    if serde_json::to_writer(&mut Capped(0), &value).is_err() {
        return Err(ReportError::TooLarge);
    }
    let Value::Object(mut object) = value else {
        return Err(ReportError::Shape("not an object".into()));
    };
    match object.remove("v") {
        Some(Value::Number(v)) if v.as_u64() == Some(1) => {}
        _ => return Err(ReportError::Version),
    }
    let kind = match object.remove("kind") {
        Some(Value::String(kind)) => kind,
        _ => return Err(ReportError::Shape("no kind".into())),
    };
    match kind.as_str() {
        "ended" if object.is_empty() => Ok(Report::Ended),
        "ended" => Err(ReportError::Shape("fields on an ended report".into())),
        "state" => state(object).map(Report::State),
        _ => Err(ReportError::Shape("unknown kind".into())),
    }
}

fn state(object: Map<String, Value>) -> Result<PageState, ReportError> {
    let raw: RawState = serde_json::from_value(Value::Object(object))
        .map_err(|err| ReportError::Shape(err.to_string()))?;
    let duration_ms = millis(raw.duration_ms);
    let mut position_ms = millis(raw.position_ms);
    if duration_ms > 0 {
        position_ms = position_ms.min(duration_ms + POSITION_SLACK_MS);
    }
    Ok(PageState {
        playing: raw.playing,
        title: clean(&raw.title),
        artist: clean(&raw.artist),
        album: clean(&raw.album),
        artwork: artwork(&raw.artwork),
        position_ms,
        duration_ms,
        can_next: raw.can_next,
        can_prev: raw.can_prev,
        available: raw.available,
    })
}

/// Control and invisible format characters dropped, at most [`MAX_TEXT_CHARS`], trimmed.
fn clean(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|ch| !ch.is_control() && !is_format_char(*ch))
        .take(MAX_TEXT_CHARS)
        .collect();
    kept.trim().to_string()
}

/// Whole milliseconds in `0..=MAX_MS`.
fn millis(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    (value.round() as u64).min(MAX_MS)
}

fn artwork(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > MAX_ARTWORK_BYTES {
        return None;
    }
    if raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    artwork_allowed(&url).then(|| url.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const ART: &str = "https://lh3.googleusercontent.com/abc=w544-h544";

    /// A report as the bridge sends it.
    pub(crate) fn bridge_report() -> Value {
        json!({
            "v": 1,
            "kind": "state",
            "playing": true,
            "title": "Testlied",
            "artist": "Die Testband",
            "album": "Testhausen",
            "artwork": ART,
            "positionMs": 12_000,
            "durationMs": 180_000,
            "canNext": true,
            "canPrev": false,
            "available": true,
        })
    }

    fn with(field: &str, value: Value) -> Value {
        let mut report = bridge_report();
        report[field] = value;
        report
    }

    fn state_of(value: Value) -> PageState {
        match parse(value) {
            Ok(Report::State(state)) => state,
            other => panic!("expected a state, got {other:?}"),
        }
    }

    #[test]
    fn report_payload_is_capped_and_validated() {
        let good = state_of(bridge_report());
        assert_eq!(good.title, "Testlied");
        assert_eq!(good.artwork.as_deref(), Some(ART));
        assert_eq!((good.position_ms, good.duration_ms), (12_000, 180_000));
        assert!(good.playing && good.available && good.can_next && !good.can_prev);
        assert_eq!(parse(json!({ "v": 1, "kind": "ended" })), Ok(Report::Ended));

        // 17 KiB, even in a field that is otherwise fine.
        let big = with("title", Value::String("x".repeat(17 * 1024)));
        assert_eq!(parse(big), Err(ReportError::TooLarge));
        let mut extra = bridge_report();
        extra["padding"] = Value::String("y".repeat(17 * 1024));
        assert_eq!(parse(extra), Err(ReportError::TooLarge));

        // Versions and kinds.
        for v in [json!(2), json!(0), json!("1"), json!(1.5), Value::Null] {
            assert_eq!(parse(with("v", v)), Err(ReportError::Version));
        }
        let mut no_version = bridge_report();
        no_version.as_object_mut().unwrap().remove("v");
        assert_eq!(parse(no_version), Err(ReportError::Version));
        assert!(matches!(
            parse(with("kind", json!("eval"))),
            Err(ReportError::Shape(_))
        ));
        assert!(matches!(
            parse(json!({ "v": 1, "kind": "ended", "title": "x" })),
            Err(ReportError::Shape(_))
        ));
        assert!(matches!(parse(json!([1, 2])), Err(ReportError::Shape(_))));
        assert!(matches!(parse(json!("state")), Err(ReportError::Shape(_))));

        // NaN arrives as null (JSON has no NaN), or as a string: refused. Missing fields,
        // wrong types and unknown fields too.
        for bad in [json!(null), json!("NaN"), json!("12"), json!([])] {
            assert!(matches!(
                parse(with("positionMs", bad)),
                Err(ReportError::Shape(_))
            ));
        }
        assert!(matches!(
            parse(with("playing", json!("true"))),
            Err(ReportError::Shape(_))
        ));
        assert!(matches!(
            parse(with("title", json!(5))),
            Err(ReportError::Shape(_))
        ));
        let mut missing = bridge_report();
        missing.as_object_mut().unwrap().remove("available");
        assert!(matches!(parse(missing), Err(ReportError::Shape(_))));
        assert!(matches!(
            parse(with("onclick", json!("x"))),
            Err(ReportError::Shape(_))
        ));

        // Numbers: clamped, rounded, never negative; the position stays near the duration.
        let state = state_of(with("positionMs", json!(-5)));
        assert_eq!(state.position_ms, 0);
        let state = state_of(with("positionMs", json!(1e300)));
        assert_eq!(state.position_ms, 180_000 + POSITION_SLACK_MS);
        let state = state_of(with("durationMs", json!(1e300)));
        assert_eq!(state.duration_ms, MAX_MS);
        let state = state_of(with("positionMs", json!(1234.6)));
        assert_eq!(state.position_ms, 1235);
        let mut unknown_length = with("durationMs", json!(0));
        unknown_length["positionMs"] = json!(1e300);
        assert_eq!(state_of(unknown_length).position_ms, MAX_MS);

        // Artwork: https on an artwork host only, else dropped (the rest of the report counts).
        for bad in [
            "javascript:alert(1)",
            "http://lh3.googleusercontent.com/abc",
            "https://evil.example/abc.jpg",
            "https://googleusercontent.com.evil.example/a.jpg",
            "https://127.0.0.1/a.jpg",
            "https://lh3.googleusercontent.com:8443/a.jpg",
            "https://user@lh3.googleusercontent.com/a.jpg",
            "data:image/png;base64,AAAA",
            "https://lh3.googleusercontent.com/a b",
            "",
        ] {
            let state = state_of(with("artwork", json!(bad)));
            assert_eq!(state.artwork, None, "{bad}");
            assert_eq!(state.title, "Testlied");
        }
        let long = format!("https://i.ytimg.com/{}", "a".repeat(MAX_ARTWORK_BYTES));
        assert_eq!(state_of(with("artwork", json!(long))).artwork, None);
        for good in [
            "https://i.ytimg.com/vi/x/hq.jpg",
            "https://yt3.ggpht.com/abc",
            "https://LH3.GoogleUserContent.com/abc",
        ] {
            assert!(
                state_of(with("artwork", json!(good))).artwork.is_some(),
                "{good}"
            );
        }

        // Strings: control and format characters dropped, capped at 256 chars.
        let state = state_of(with(
            "title",
            json!("Test\u{0}lied\n\u{202E}gnos\u{200B}\u{7}"),
        ));
        assert_eq!(state.title, "Testliedgnos");
        let state = state_of(with("artist", json!("\u{e4}".repeat(300))));
        assert_eq!(state.artist.chars().count(), MAX_TEXT_CHARS);
        let state = state_of(with("album", json!("  Testhausen  ")));
        assert_eq!(state.album, "Testhausen");
    }
}
