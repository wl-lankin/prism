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
/// The cap for a queue report: 200 entries of typical length are about 60 KiB, and the
/// bridge leaves out the tail of a queue that would not fit.
pub const MAX_QUEUE_REPORT_BYTES: usize = 256 * 1024;
pub const MAX_QUEUE_ITEMS: usize = 200;
/// The cap for a lyrics report; the bridge cuts the text to fit.
pub const MAX_LYRICS_REPORT_BYTES: usize = 64 * 1024;
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
    Queue(PageQueue),
    /// The page's answer to one of Prism's lyrics requests.
    Lyrics(PageLyrics),
}

/// The lyrics YouTube Music's own API has for one video, as the page fetched them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageLyrics {
    /// The number of the request this answers.
    pub request: u32,
    /// Always a video id.
    pub video_id: String,
    /// Raw text, bounded only by the report's cap; `Lyrics::plain` cleans it. `None`: no lyrics.
    pub text: Option<String>,
    /// For example "Source: LyricFind", cleaned like a title.
    pub provider: Option<String>,
    /// The page could not ask YouTube Music (network, status, an unknown answer); never a miss.
    pub failed: bool,
}

/// YouTube Music's queue as its page shows it, cleaned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageQueue {
    pub items: Vec<PageQueueItem>,
    /// The page has its queue element (YouTube Music may render it only once its player
    /// page was opened). Without it there are no items.
    pub has_queue: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageQueueItem {
    pub title: String,
    pub byline: String,
    /// An https URL on an artwork host; `None` for anything else.
    pub thumb: Option<String>,
    /// At most one entry is current.
    pub current: bool,
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
    /// The playing video's id; `None` when the page sent none or something else.
    pub video_id: Option<String>,
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
    video_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLyrics {
    request: u32,
    video_id: String,
    text: Option<String>,
    provider: Option<String>,
    failed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawQueue {
    items: Vec<RawQueueItem>,
    has_queue: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQueueItem {
    title: String,
    byline: String,
    thumb: String,
    current: bool,
}

/// Counts bytes and fails once more than its cap were written, so an oversized report is
/// never serialized in full.
struct Capped {
    written: usize,
    cap: usize,
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written += buf.len();
        if self.written > self.cap {
            return Err(std::io::Error::other("too large"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn parse(value: Value) -> Result<Report, ReportError> {
    let cap = match value.get("kind").and_then(Value::as_str) {
        Some("queue") => MAX_QUEUE_REPORT_BYTES,
        Some("lyrics") => MAX_LYRICS_REPORT_BYTES,
        _ => MAX_REPORT_BYTES,
    };
    if serde_json::to_writer(&mut Capped { written: 0, cap }, &value).is_err() {
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
        "queue" => queue(object).map(Report::Queue),
        "lyrics" => lyrics(object).map(Report::Lyrics),
        _ => Err(ReportError::Shape("unknown kind".into())),
    }
}

/// YouTube's video ids: exactly 11 of `A-Z a-z 0-9 _ -`.
pub fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn lyrics(object: Map<String, Value>) -> Result<PageLyrics, ReportError> {
    // serde would take a missing `Option` field as `None`; the bridge always sends both.
    for field in ["text", "provider"] {
        if !object.contains_key(field) {
            return Err(ReportError::Shape(format!("missing field `{field}`")));
        }
    }
    let raw: RawLyrics = serde_json::from_value(Value::Object(object))
        .map_err(|err| ReportError::Shape(err.to_string()))?;
    if !is_video_id(&raw.video_id) {
        return Err(ReportError::Shape("not a video id".into()));
    }
    if raw.failed && raw.text.is_some() {
        return Err(ReportError::Shape("lyrics on a failed answer".into()));
    }
    Ok(PageLyrics {
        request: raw.request,
        video_id: raw.video_id,
        text: raw.text,
        provider: raw
            .provider
            .map(|provider| clean(&provider))
            .filter(|provider| !provider.is_empty()),
        failed: raw.failed,
    })
}

fn queue(object: Map<String, Value>) -> Result<PageQueue, ReportError> {
    let raw: RawQueue = serde_json::from_value(Value::Object(object))
        .map_err(|err| ReportError::Shape(err.to_string()))?;
    if raw.items.len() > MAX_QUEUE_ITEMS {
        return Err(ReportError::Shape("too many queue entries".into()));
    }
    if !raw.has_queue && !raw.items.is_empty() {
        return Err(ReportError::Shape("queue entries without a queue".into()));
    }
    let mut seen_current = false;
    let items = raw
        .items
        .into_iter()
        .map(|item| {
            let current = item.current && !seen_current;
            seen_current |= current;
            PageQueueItem {
                title: clean(&item.title),
                byline: clean(&item.byline),
                thumb: artwork(&item.thumb),
                current,
            }
        })
        .collect();
    Ok(PageQueue {
        items,
        has_queue: raw.has_queue,
    })
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
        video_id: is_video_id(&raw.video_id).then_some(raw.video_id),
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
    // Parsing percent-encodes, so the URL can grow past the cap.
    (artwork_allowed(&url) && url.as_str().len() <= MAX_ARTWORK_BYTES).then(|| url.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const ART: &str = "https://lh3.googleusercontent.com/abc=w544-h544";
    /// An invented video id in YouTube's shape.
    pub(crate) const VIDEO: &str = "testVideo01";

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
            "videoId": VIDEO,
        })
    }

    /// A lyrics answer as the bridge sends it.
    pub(crate) fn lyrics_report(request: u32, text: Option<&str>) -> Value {
        json!({
            "v": 1,
            "kind": "lyrics",
            "request": request,
            "videoId": VIDEO,
            "text": text,
            "provider": text.map(|_| "Source: Testquelle"),
            "failed": false,
        })
    }

    fn lyrics_of(value: Value) -> PageLyrics {
        match parse(value) {
            Ok(Report::Lyrics(lyrics)) => lyrics,
            other => panic!("expected lyrics, got {other:?}"),
        }
    }

    #[test]
    fn state_carries_a_checked_video_id() {
        assert_eq!(state_of(bridge_report()).video_id.as_deref(), Some(VIDEO));
        for good in ["abc-DEF_123", "___________", "0123456789a"] {
            assert_eq!(
                state_of(with("videoId", json!(good))).video_id.as_deref(),
                Some(good)
            );
        }
        // Not an id: dropped, the rest of the report still counts.
        for bad in [
            "",
            "short",
            "testVideo012",
            "testVideo0!",
            "test/ideo01",
            "testVide\u{e4}1",
            "testVideo0\n",
            " testVideo0",
            "../../etc/p",
        ] {
            let state = state_of(with("videoId", json!(bad)));
            assert_eq!(state.video_id, None, "{bad:?}");
            assert_eq!(state.title, "Testlied");
        }
        // Missing or mistyped: the report has the wrong shape.
        let mut missing = bridge_report();
        missing.as_object_mut().unwrap().remove("videoId");
        assert!(matches!(parse(missing), Err(ReportError::Shape(_))));
        for bad in [json!(null), json!(5), json!(["testVideo01"])] {
            assert!(matches!(
                parse(with("videoId", bad)),
                Err(ReportError::Shape(_))
            ));
        }
    }

    #[test]
    fn lyrics_report_is_strict_and_capped() {
        let lyrics = lyrics_of(lyrics_report(7, Some("Eins\nZwei")));
        assert_eq!(
            lyrics,
            PageLyrics {
                request: 7,
                video_id: VIDEO.into(),
                text: Some("Eins\nZwei".into()),
                provider: Some("Source: Testquelle".into()),
                failed: false,
            }
        );
        let none = lyrics_of(lyrics_report(8, None));
        assert_eq!((none.text, none.provider, none.failed), (None, None, false));
        let mut failed = lyrics_report(9, None);
        failed["failed"] = json!(true);
        assert!(lyrics_of(failed).failed);

        // The text stays raw (Lyrics::plain cleans it); the provider is cleaned like a title.
        let mut dirty = lyrics_report(1, Some("a\u{0}b\r\nc"));
        dirty["provider"] = json!(format!("  Source:\u{202E} X{}", "y".repeat(400)));
        let dirty = lyrics_of(dirty);
        assert_eq!(dirty.text.as_deref(), Some("a\u{0}b\r\nc"));
        let provider = dirty.provider.unwrap();
        assert!(provider.starts_with("Source: X"));
        assert_eq!(provider.chars().count(), MAX_TEXT_CHARS - 2);
        let mut blank = lyrics_report(1, Some("x"));
        blank["provider"] = json!(" \u{200B} ");
        assert_eq!(lyrics_of(blank).provider, None);

        // A full lyrics text fits its own cap, which is larger than a state's; past it, refused.
        let long = "Zeile aus Testhausen\n".repeat(2_500);
        assert!(long.len() > MAX_REPORT_BYTES && long.len() < MAX_LYRICS_REPORT_BYTES - 1024);
        assert_eq!(
            lyrics_of(lyrics_report(1, Some(&long))).text.as_deref(),
            Some(long.as_str())
        );
        let huge = "x".repeat(MAX_LYRICS_REPORT_BYTES);
        assert_eq!(
            parse(lyrics_report(1, Some(&huge))),
            Err(ReportError::TooLarge)
        );
        // A state report does not get the lyrics cap by claiming it elsewhere.
        let mut big_state = bridge_report();
        big_state["title"] = Value::String("x".repeat(17 * 1024));
        big_state["text"] = json!("lyrics");
        assert_eq!(parse(big_state), Err(ReportError::TooLarge));

        // The video id must be one.
        for bad in [
            "",
            "short",
            "testVideo012",
            "test/ideo01",
            "testVideo0\u{0}",
        ] {
            let mut report = lyrics_report(1, Some("x"));
            report["videoId"] = json!(bad);
            assert!(
                matches!(parse(report), Err(ReportError::Shape(_))),
                "{bad:?}"
            );
        }

        // The request id: a whole number in u32.
        for bad in [
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(null),
            json!(u64::from(u32::MAX) + 1),
        ] {
            let mut report = lyrics_report(1, Some("x"));
            report["request"] = bad.clone();
            assert!(matches!(parse(report), Err(ReportError::Shape(_))), "{bad}");
        }
        assert_eq!(lyrics_of(lyrics_report(u32::MAX, None)).request, u32::MAX);

        // Shape: unknown, missing and mistyped fields refuse the whole report.
        let mut extra = lyrics_report(1, Some("x"));
        extra["html"] = json!("<b>x</b>");
        assert!(matches!(parse(extra), Err(ReportError::Shape(_))));
        for field in ["request", "videoId", "text", "provider", "failed"] {
            let mut missing = lyrics_report(1, Some("x"));
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                matches!(parse(missing), Err(ReportError::Shape(_))),
                "{field}"
            );
        }
        for (field, value) in [
            ("text", json!(5)),
            ("text", json!(["a"])),
            ("provider", json!(5)),
            ("failed", json!("false")),
            ("failed", Value::Null),
        ] {
            let mut typed = lyrics_report(1, Some("x"));
            typed[field] = value;
            assert!(
                matches!(parse(typed), Err(ReportError::Shape(_))),
                "{field}"
            );
        }
        // A failure carries no text.
        let mut both = lyrics_report(1, Some("x"));
        both["failed"] = json!(true);
        assert!(matches!(parse(both), Err(ReportError::Shape(_))));
        let mut version = lyrics_report(1, None);
        version["v"] = json!(2);
        assert_eq!(parse(version), Err(ReportError::Version));
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
        // Under the cap as sent, over it once parsing percent-encodes the path.
        let grows = format!("https://i.ytimg.com/{}", "\u{e4}".repeat(1_000));
        assert!(grows.len() <= MAX_ARTWORK_BYTES);
        assert_eq!(state_of(with("artwork", json!(grows))).artwork, None);
        let fits = format!("https://i.ytimg.com/{}", "\u{e4}".repeat(300));
        let parsed = state_of(with("artwork", json!(fits))).artwork.unwrap();
        assert!(parsed.contains("%C3%A4") && parsed.len() <= MAX_ARTWORK_BYTES);
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

    pub(crate) const THUMB: &str = "https://i.ytimg.com/vi/abc/default.jpg";

    /// A queue report as the bridge sends it: three entries, the second one current.
    pub(crate) fn queue_report() -> Value {
        json!({
            "v": 1,
            "kind": "queue",
            "hasQueue": true,
            "items": [
                { "title": "Eins", "byline": "Die Testband", "thumb": THUMB, "current": false },
                { "title": "Zwei", "byline": "Die Testband", "thumb": "", "current": true },
                { "title": "Drei", "byline": "", "thumb": THUMB, "current": false },
            ],
        })
    }

    fn queue_of(value: Value) -> PageQueue {
        match parse(value) {
            Ok(Report::Queue(queue)) => queue,
            other => panic!("expected a queue, got {other:?}"),
        }
    }

    fn item(title: &str, thumb: &str, current: bool) -> Value {
        json!({ "title": title, "byline": "", "thumb": thumb, "current": current })
    }

    fn with_items(items: Vec<Value>) -> Value {
        json!({ "v": 1, "kind": "queue", "hasQueue": true, "items": items })
    }

    #[test]
    fn queue_report_is_strict_and_capped() {
        let queue = queue_of(queue_report());
        assert!(queue.has_queue);
        let titles: Vec<&str> = queue.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Eins", "Zwei", "Drei"]);
        assert_eq!(queue.items[0].thumb.as_deref(), Some(THUMB));
        assert_eq!(queue.items[1].thumb, None);
        assert_eq!(queue.items[0].byline, "Die Testband");
        let current: Vec<bool> = queue.items.iter().map(|i| i.current).collect();
        assert_eq!(current, [false, true, false]);

        // No queue element on the page.
        let none = queue_of(json!({ "v": 1, "kind": "queue", "hasQueue": false, "items": [] }));
        assert_eq!(none, PageQueue::default());
        // Entries without a queue element cannot be.
        let mut bad = queue_report();
        bad["hasQueue"] = json!(false);
        assert!(matches!(parse(bad), Err(ReportError::Shape(_))));

        // Shape: unknown, missing and mistyped fields refuse the whole report.
        let mut extra = queue_report();
        extra["onclick"] = json!("x");
        assert!(matches!(parse(extra), Err(ReportError::Shape(_))));
        let mut item_extra = queue_report();
        item_extra["items"][0]["href"] = json!("https://evil.example/");
        assert!(matches!(parse(item_extra), Err(ReportError::Shape(_))));
        let mut missing = queue_report();
        missing["items"][1]
            .as_object_mut()
            .unwrap()
            .remove("current");
        assert!(matches!(parse(missing), Err(ReportError::Shape(_))));
        let mut no_flag = queue_report();
        no_flag.as_object_mut().unwrap().remove("hasQueue");
        assert!(matches!(parse(no_flag), Err(ReportError::Shape(_))));
        for (field, value) in [
            ("title", json!(5)),
            ("current", json!("true")),
            ("thumb", Value::Null),
        ] {
            let mut typed = queue_report();
            typed["items"][0][field] = value;
            assert!(
                matches!(parse(typed), Err(ReportError::Shape(_))),
                "{field}"
            );
        }
        let mut not_a_list = queue_report();
        not_a_list["items"] = json!({ "0": item("x", "", false) });
        assert!(matches!(parse(not_a_list), Err(ReportError::Shape(_))));
        let mut version = queue_report();
        version["v"] = json!(2);
        assert_eq!(parse(version), Err(ReportError::Version));

        // At most 200 entries.
        let full = with_items(
            (0..MAX_QUEUE_ITEMS)
                .map(|n| item(&format!("Lied {n}"), "", false))
                .collect(),
        );
        assert_eq!(queue_of(full).items.len(), MAX_QUEUE_ITEMS);
        let over = with_items(
            (0..=MAX_QUEUE_ITEMS)
                .map(|n| item(&format!("Lied {n}"), "", false))
                .collect(),
        );
        assert!(matches!(parse(over), Err(ReportError::Shape(_))));

        // A full queue fits its own cap, which is larger than a state's; past it, refused.
        let long = with_items(
            (0..MAX_QUEUE_ITEMS)
                .map(|_| {
                    json!({
                        "title": "t".repeat(MAX_TEXT_CHARS),
                        "byline": "b".repeat(MAX_TEXT_CHARS),
                        "thumb": format!("https://i.ytimg.com/{}", "u".repeat(200)),
                        "current": false,
                    })
                })
                .collect(),
        );
        assert!(serde_json::to_string(&long).unwrap().len() > MAX_REPORT_BYTES);
        assert_eq!(queue_of(long).items.len(), MAX_QUEUE_ITEMS);
        let mut huge = queue_report();
        huge["items"][0]["title"] = Value::String("x".repeat(MAX_QUEUE_REPORT_BYTES));
        assert_eq!(parse(huge), Err(ReportError::TooLarge));
        // A state report does not get the queue's cap by claiming it elsewhere.
        let mut big_state = bridge_report();
        big_state["title"] = Value::String("x".repeat(17 * 1024));
        assert_eq!(parse(big_state), Err(ReportError::TooLarge));

        // Strings cleaned and capped as in a state report.
        let dirty = with_items(vec![json!({
            "title": "  Test\u{0}lied\u{202E}  ",
            "byline": "\u{e4}".repeat(300),
            "thumb": "",
            "current": false,
        })]);
        let cleaned = queue_of(dirty);
        assert_eq!(cleaned.items[0].title, "Testlied");
        assert_eq!(cleaned.items[0].byline.chars().count(), MAX_TEXT_CHARS);

        // Thumbs: https on an artwork host only, else dropped (the entry stays).
        for bad in [
            "javascript:alert(1)",
            "http://i.ytimg.com/vi/x/default.jpg",
            "https://evil.example/a.jpg",
            "https://i.ytimg.com:8443/a.jpg",
            "https://user@i.ytimg.com/a.jpg",
            "data:image/png;base64,AAAA",
        ] {
            let queue = queue_of(with_items(vec![item("x", bad, false)]));
            assert_eq!(queue.items[0].thumb, None, "{bad}");
            assert_eq!(queue.items[0].title, "x");
        }
        let long_thumb = format!("https://i.ytimg.com/{}", "a".repeat(MAX_ARTWORK_BYTES));
        assert_eq!(
            queue_of(with_items(vec![item("x", &long_thumb, false)])).items[0].thumb,
            None
        );
        for good in [
            "https://lh3.googleusercontent.com/abc=w60-h60",
            "https://yt3.ggpht.com/a",
        ] {
            let queue = queue_of(with_items(vec![item("x", good, false)]));
            assert_eq!(queue.items[0].thumb.as_deref(), Some(good));
        }

        // Only the first entry marked current counts.
        let two = with_items(vec![
            item("a", "", false),
            item("b", "", true),
            item("c", "", true),
        ]);
        let current: Vec<bool> = queue_of(two).items.iter().map(|i| i.current).collect();
        assert_eq!(current, [false, true, false]);
    }
}
