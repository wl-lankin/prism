//! LRC parser. Accepts the common dialects: `[mm:ss.xx]` tags (also `.x`, `.xxx`, `:xx`),
//! several tags per line, `[offset:]`, metadata tags and enhanced `<mm:ss.xx>` word tags.

use super::{Lyrics, LyricsSource};

const METADATA_KEYS: [&str; 9] = ["ar", "ti", "al", "au", "by", "length", "re", "ve", "offset"];

/// Room for repeated-tag lines beyond what `Lyrics::synced` keeps after sorting.
const MAX_TIMED: usize = 4 * super::MAX_LINES;

/// Timed lines become synced lyrics. Text without any time tag becomes plain lyrics.
pub fn parse(text: &str, source: LyricsSource) -> Option<Lyrics> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut timed: Vec<(i64, String)> = Vec::new();
    let mut offset_ms: i64 = 0;
    let mut saw_tag = false;

    for line in text.lines() {
        let mut rest = line.trim_start();
        let mut stamps: Vec<i64> = Vec::new();
        while let Some(tag) = leading_tag(rest) {
            match tag.kind {
                Tag::Time(ms) => {
                    saw_tag = true;
                    stamps.push(ms);
                }
                Tag::Offset(ms) => offset_ms = ms,
                Tag::Meta => {}
            }
            rest = rest[tag.len..].trim_start();
        }
        if stamps.is_empty() {
            continue;
        }
        // Cut before repeating per tag: a hostile line must not multiply its text.
        let line_text: String = strip_word_tags(rest)
            .chars()
            .take(super::MAX_LINE_CHARS)
            .collect();
        for ms in stamps {
            if timed.len() >= MAX_TIMED {
                break;
            }
            timed.push((ms, line_text.clone()));
        }
    }

    if !saw_tag {
        return Lyrics::plain(source, &plain_text(text));
    }
    let timed = timed
        .into_iter()
        .map(|(ms, t)| (ms.saturating_sub(offset_ms).max(0) as u64, t))
        .collect();
    Lyrics::synced(source, timed)
}

/// Untimed text without its metadata lines (`[ar:...]` and friends).
fn plain_text(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let line = line.trim();
            !leading_tag(line).is_some_and(|t| {
                matches!(t.kind, Tag::Meta | Tag::Offset(_)) && t.len == line.len()
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

enum Tag {
    Time(i64),
    Offset(i64),
    Meta,
}

struct Leading {
    kind: Tag,
    len: usize,
}

/// A `[...]` tag at the start of `s`: a time stamp or a known metadata tag. Anything else
/// (`[Chorus]`) is text.
fn leading_tag(s: &str) -> Option<Leading> {
    let inner_end = s.strip_prefix('[')?.find(']')?;
    let inner = &s[1..1 + inner_end];
    let len = inner_end + 2;
    if let Some(ms) = time_stamp(inner) {
        return Some(Leading {
            kind: Tag::Time(ms),
            len,
        });
    }
    let (key, value) = inner.split_once(':')?;
    let key = key.trim().to_ascii_lowercase();
    if key == "offset" {
        let offset = value.trim().trim_start_matches('+').parse::<i64>().ok()?;
        return Some(Leading {
            kind: Tag::Offset(offset),
            len,
        });
    }
    METADATA_KEYS.contains(&key.as_str()).then_some(Leading {
        kind: Tag::Meta,
        len,
    })
}

/// `mm:ss`, `mm:ss.f`, `mm:ss.ff`, `mm:ss.fff` or `mm:ss:ff` (no brackets), in milliseconds.
fn time_stamp(s: &str) -> Option<i64> {
    let (minutes, rest) = s.split_once(':')?;
    if minutes.is_empty() || minutes.len() > 4 || !minutes.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((sec, frac)) => (sec, Some(frac)),
        None => (rest, None),
    };
    if seconds.is_empty() || seconds.len() > 2 || !seconds.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: i64 = seconds.parse().ok()?;
    if seconds > 59 {
        return None;
    }
    let fraction_ms = match fraction {
        None => 0,
        Some(f) if (1..=3).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()) => {
            f.parse::<i64>().ok()? * 10_i64.pow(3 - f.len() as u32)
        }
        Some(_) => return None,
    };
    Some(minutes.parse::<i64>().ok()? * 60_000 + seconds * 1000 + fraction_ms)
}

/// Removes enhanced LRC word tags like `<00:12.34>` and trims.
fn strip_word_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        // The longest tag, `<9999:59.999>`, closes within 11 bytes; a wider search is quadratic.
        let end = after.bytes().take(12).position(|b| b == b'>');
        match end {
            Some(end) if time_stamp(&after[..end]).is_some() => rest = &after[end + 1..],
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced(text: &str) -> Vec<(u64, String)> {
        let l = parse(text, LyricsSource::LrcFile).expect("lyrics");
        assert!(l.synced, "expected synced lyrics");
        l.lines
            .into_iter()
            .map(|l| (l.start_ms.unwrap(), l.text))
            .collect()
    }

    fn row(ms: u64, text: &str) -> (u64, String) {
        (ms, text.to_string())
    }

    #[test]
    fn parses_every_fraction_form() {
        let got = synced("[00:01]a\n[00:02.5]b\n[00:03.25]c\n[00:04.125]d\n[00:05:50]e");
        assert_eq!(
            got,
            [
                row(1_000, "a"),
                row(2_500, "b"),
                row(3_250, "c"),
                row(4_125, "d"),
                row(5_500, "e")
            ]
        );
    }

    #[test]
    fn minutes_above_59_work() {
        assert_eq!(
            synced("[75:30.00]late"),
            [row(75 * 60_000 + 30_000, "late")]
        );
    }

    #[test]
    fn several_tags_repeat_the_line_and_sort() {
        let got = synced("[00:30.00][00:10.00]Refrain\n[00:20.00]Strophe");
        assert_eq!(
            got,
            [
                row(10_000, "Refrain"),
                row(20_000, "Strophe"),
                row(30_000, "Refrain")
            ]
        );
    }

    #[test]
    fn positive_offset_makes_lines_earlier_and_clamps() {
        let got = synced("[offset:+500]\n[00:00.20]a\n[00:01.00]b");
        assert_eq!(got, [row(0, "a"), row(500, "b")]);
        let got = synced("[offset:-250]\n[00:01.00]b");
        assert_eq!(got, [row(1_250, "b")]);
    }

    #[test]
    fn offset_after_the_lines_still_applies() {
        assert_eq!(synced("[00:02.00]a\n[offset:1000]"), [row(1_000, "a")]);
    }

    #[test]
    fn metadata_tags_are_ignored() {
        let text = "[ar:Testband]\n[ti:Teststück]\n[al:Album]\n[by:me]\n[length:03:20]\n[re:tool]\n[ve:1.0]\n[xx:other]\n[00:01.00]Zeile";
        assert_eq!(synced(text), [row(1_000, "Zeile")]);
    }

    #[test]
    fn enhanced_word_tags_are_removed() {
        let got = synced("[00:01.00]<00:01.00>Eins <00:01.50>zwei <00:02.00>drei");
        assert_eq!(got, [row(1_000, "Eins zwei drei")]);
        let got = synced("[00:01.00]a < b > c");
        assert_eq!(got, [row(1_000, "a < b > c")]);
    }

    #[test]
    fn bom_crlf_and_blank_lines() {
        let got = synced("\u{feff}[00:01.00]a\r\n\r\n[00:02.00]b\r\n");
        assert_eq!(got, [row(1_000, "a"), row(2_000, "b")]);
    }

    #[test]
    fn blank_timed_lines_stay() {
        let got = synced("[00:01.00]a\n[00:02.00]\n[00:03.00]b");
        assert_eq!(got, [row(1_000, "a"), row(2_000, ""), row(3_000, "b")]);
    }

    #[test]
    fn untimed_lines_are_ignored_when_times_exist() {
        let got = synced("Titelzeile\n[00:01.00]a\n[Chorus]\n[00:02.00]b");
        assert_eq!(got, [row(1_000, "a"), row(2_000, "b")]);
    }

    #[test]
    fn text_without_time_tags_is_plain() {
        let l = parse(
            "[ar:Testband]\r\nErste Zeile\r\n[Chorus]\r\nZweite Zeile\r\n",
            LyricsSource::Embedded,
        )
        .unwrap();
        assert!(!l.synced);
        assert_eq!(l.source, LyricsSource::Embedded);
        let texts: Vec<_> = l.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["Erste Zeile", "[Chorus]", "Zweite Zeile"]);
    }

    #[test]
    fn nothing_usable_is_none() {
        assert_eq!(parse("", LyricsSource::LrcFile), None);
        assert_eq!(parse("[ar:x]\n\n", LyricsSource::LrcFile), None);
        assert_eq!(
            parse("[00:01.00]\n[00:02.00]  ", LyricsSource::LrcFile),
            None
        );
    }

    #[test]
    fn malformed_tags_are_text() {
        for bad in [
            "[0:1:2:3]x",
            "[00:75.00]x",
            "[00:01.1234]x",
            // Overlong minutes must not overflow.
            "[99999999999999999999:01]x",
        ] {
            let l = parse(bad, LyricsSource::LrcFile).unwrap();
            assert!(!l.synced, "{bad}");
        }
    }

    #[test]
    fn hostile_input_stays_small_and_fast() {
        let start = std::time::Instant::now();
        let line = format!("{}{}", "[0:01]".repeat(10_000), "x".repeat(100_000));
        let l = parse(&line, LyricsSource::LrcFile).unwrap();
        assert!(l.lines.len() <= crate::lyrics::MAX_LINES);
        assert!(l
            .lines
            .iter()
            .all(|l| l.text.chars().count() <= crate::lyrics::MAX_LINE_CHARS));

        let many = "[0:01]".repeat(200_000) + "y";
        assert!(parse(&many, LyricsSource::LrcFile).unwrap().lines.len() <= 2000);

        let angle = format!("[0:01]{}", "<".repeat(300_000));
        parse(&angle, LyricsSource::LrcFile);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn extreme_offsets_do_not_overflow() {
        for offset in [
            "-9223372036854775807",
            "9223372036854775807",
            "-9223372036854775808",
        ] {
            let l = parse(
                &format!("[offset:{offset}]\n[00:01.00]a"),
                LyricsSource::LrcFile,
            )
            .unwrap();
            assert_eq!(l.lines.len(), 1);
        }
        let got = synced("[offset:-9223372036854775807]\n[00:01.00]a");
        assert_eq!(got.len(), 1);
        assert_eq!(
            synced("[offset:9223372036854775807]\n[00:01.00]a"),
            [row(0, "a")]
        );
    }

    #[test]
    fn word_tags_leave_single_spaces() {
        let got = synced("[00:01.00]<00:01.00> Eins <00:01.50> zwei  <00:02.00>drei");
        assert_eq!(got, [row(1_000, "Eins zwei drei")]);
    }

    #[test]
    fn section_headings_stay_text_in_plain_lyrics() {
        let l = parse(
            "[Verse 1]\nEins\n[Verse 1: Name]\nZwei",
            LyricsSource::LrcFile,
        )
        .unwrap();
        assert!(!l.synced);
        let texts: Vec<_> = l.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["[Verse 1]", "Eins", "[Verse 1: Name]", "Zwei"]);
    }
}
