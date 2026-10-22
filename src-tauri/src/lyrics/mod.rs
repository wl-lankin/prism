//! Song lyrics: one shape for every source (sidecar .lrc, embedded tags, Navidrome, LRCLIB,
//! YouTube Music). Every source builds its result through `Lyrics::plain` or `Lyrics::synced`,
//! so text from files, servers and web pages is cleaned and bounded in one place.

use serde::{Deserialize, Serialize};

pub mod cache;
pub mod local;
pub mod lrc;
pub mod lrclib;

pub const MAX_LINES: usize = 2000;
pub const MAX_LINE_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LyricsSource {
    LrcFile,
    Embedded,
    Navidrome,
    Lrclib,
    Youtube,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// Set on every line of synced lyrics, never on plain lyrics.
    pub start_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub source: LyricsSource,
    pub synced: bool,
    /// The source says the track has no vocals; `lines` is empty.
    pub instrumental: bool,
    pub lines: Vec<Line>,
}

impl Lyrics {
    /// Plain text, one line per line. Blank lines at either end go, runs of blank lines
    /// become one. `None` when no text is left.
    pub fn plain(source: LyricsSource, text: &str) -> Option<Self> {
        let mut lines: Vec<Line> = Vec::new();
        for raw in text.lines() {
            let text = clean(raw);
            if text.is_empty() && lines.last().is_none_or(|l| l.text.is_empty()) {
                continue;
            }
            lines.push(Line {
                start_ms: None,
                text,
            });
            if lines.len() == MAX_LINES {
                break;
            }
        }
        while lines.last().is_some_and(|l| l.text.is_empty()) {
            lines.pop();
        }
        (!lines.is_empty()).then_some(Self {
            source,
            synced: false,
            instrumental: false,
            lines,
        })
    }

    /// Timed lines in any order. Blank timed lines stay: they end the line before. `None`
    /// when every line is blank.
    pub fn synced(source: LyricsSource, timed: Vec<(u64, String)>) -> Option<Self> {
        let mut timed = timed;
        timed.sort_by_key(|(ms, _)| *ms);
        let lines: Vec<Line> = timed
            .into_iter()
            .take(MAX_LINES)
            .map(|(ms, text)| Line {
                start_ms: Some(ms),
                text: clean(&text),
            })
            .collect();
        lines.iter().any(|l| !l.text.is_empty()).then_some(Self {
            source,
            synced: true,
            instrumental: false,
            lines,
        })
    }

    pub fn instrumental(source: LyricsSource) -> Self {
        Self {
            source,
            synced: false,
            instrumental: true,
            lines: Vec::new(),
        }
    }
}

/// One display line: no control characters, no outer whitespace, at most `MAX_LINE_CHARS`.
fn clean(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_LINE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(l: &Lyrics) -> Vec<&str> {
        l.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn plain_trims_and_collapses_blank_lines() {
        let l = Lyrics::plain(LyricsSource::Lrclib, "\r\n\nOne\r\n\n\n  Two \n\n").unwrap();
        assert_eq!(texts(&l), ["One", "", "Two"]);
        assert!(!l.synced && !l.instrumental);
        assert!(l.lines.iter().all(|l| l.start_ms.is_none()));
    }

    #[test]
    fn plain_without_text_is_none() {
        assert_eq!(Lyrics::plain(LyricsSource::Embedded, " \n\t\n"), None);
        assert_eq!(Lyrics::plain(LyricsSource::Embedded, ""), None);
    }

    #[test]
    fn control_characters_are_removed_and_lines_capped() {
        let long = "x".repeat(MAX_LINE_CHARS + 50);
        let l =
            Lyrics::plain(LyricsSource::Youtube, &format!("a\u{0}b\u{1b}[31m\n{long}")).unwrap();
        assert_eq!(l.lines[0].text, "ab[31m");
        assert_eq!(l.lines[1].text.chars().count(), MAX_LINE_CHARS);

        let many = "line\n".repeat(MAX_LINES + 10);
        assert_eq!(
            Lyrics::plain(LyricsSource::Youtube, &many)
                .unwrap()
                .lines
                .len(),
            MAX_LINES
        );
    }

    #[test]
    fn synced_sorts_and_keeps_blank_timed_lines() {
        let l = Lyrics::synced(
            LyricsSource::LrcFile,
            vec![
                (5_000, "Two".into()),
                (1_000, " One ".into()),
                (9_000, String::new()),
            ],
        )
        .unwrap();
        assert!(l.synced);
        assert_eq!(texts(&l), ["One", "Two", ""]);
        assert_eq!(
            l.lines.iter().map(|l| l.start_ms).collect::<Vec<_>>(),
            [Some(1_000), Some(5_000), Some(9_000)]
        );
    }

    #[test]
    fn synced_without_text_is_none() {
        assert_eq!(
            Lyrics::synced(LyricsSource::Lrclib, vec![(0, " ".into())]),
            None
        );
        assert_eq!(Lyrics::synced(LyricsSource::Lrclib, Vec::new()), None);
    }

    #[test]
    fn serializes_camel_case() {
        let l = Lyrics::synced(LyricsSource::LrcFile, vec![(1, "a".into())]).unwrap();
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["source"], "lrcFile");
        assert_eq!(json["lines"][0]["startMs"], 1);
        assert_eq!(
            serde_json::to_value(Lyrics::instrumental(LyricsSource::Lrclib)).unwrap()
                ["instrumental"],
            true
        );
    }
}
