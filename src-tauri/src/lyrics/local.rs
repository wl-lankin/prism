//! Lyrics that live next to or inside an audio file: a sidecar `.lrc` and embedded tags.

use std::fs::File;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use lofty::file::{FileType, TaggedFileExt};
use lofty::id3::v2::{Frame, FrameFlags, SynchronizedTextFrame, TimestampFormat};
use lofty::mpeg::MpegFile;
use lofty::prelude::AudioFile;
use lofty::tag::ItemKey;

use super::{lrc, Lyrics, LyricsSource};

const MAX_SIDECAR_BYTES: u64 = 512 * 1024;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LocalLyrics {
    pub synced: Option<Lyrics>,
    pub plain: Option<Lyrics>,
}

/// Lyrics of a local file: the sidecar first, then the embedded tags. Never fails; a file
/// that cannot be read has no lyrics.
pub fn read(path: &Path) -> LocalLyrics {
    let sidecar = find_sidecar(path)
        .and_then(|p| read_limited(&p))
        .and_then(|bytes| lrc::parse(&decode_text(&bytes), LyricsSource::LrcFile));
    // A broken file must never take the player down, whatever the tag parser does.
    let embedded = catch_unwind(AssertUnwindSafe(|| read_embedded(path))).unwrap_or(None);

    let (sidecar_synced, sidecar_plain) = split(sidecar);
    let (embedded_synced, embedded_plain) = split(embedded);
    LocalLyrics {
        synced: sidecar_synced.or(embedded_synced),
        plain: sidecar_plain.or(embedded_plain),
    }
}

fn split(lyrics: Option<Lyrics>) -> (Option<Lyrics>, Option<Lyrics>) {
    match lyrics {
        Some(l) if l.synced => (Some(l), None),
        other => (None, other),
    }
}

/// `<stem>.lrc` next to the file; the extension may be in any case.
fn find_sidecar(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    let stem = path.file_stem()?;
    let exact = path.with_extension("lrc");
    if exact.is_file() {
        return Some(exact);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_stem() == Some(stem)
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lrc"))
                && p.is_file()
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_SIDECAR_BYTES).then_some(bytes)
}

/// UTF-16 with a BOM, else UTF-8 with replacement characters.
fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], big: bool| {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| {
                if big {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn read_embedded(path: &Path) -> Option<Lyrics> {
    let tagged = lofty::read_from_path(path).ok()?;

    let mut plain: Option<Lyrics> = None;
    let mut synced = if tagged.file_type() == FileType::Mpeg {
        sylt_lyrics(path)
    } else {
        None
    };
    for tag in tagged.tags() {
        for key in [ItemKey::UnsyncLyrics, ItemKey::Lyrics] {
            for text in tag.get_strings(key) {
                match lrc::parse(text, LyricsSource::Embedded) {
                    Some(l) if l.synced => {
                        synced.get_or_insert(l);
                    }
                    Some(l) => {
                        plain.get_or_insert(l);
                    }
                    None => {}
                }
            }
        }
    }
    synced.or(plain)
}

/// ID3 SYLT frames with millisecond timestamps. The generic tag view drops them, so the
/// MPEG file is read again for its raw ID3v2 frames.
fn sylt_lyrics(path: &Path) -> Option<Lyrics> {
    let mut file = File::open(path).ok()?;
    let mpeg = MpegFile::read_from(&mut file, lofty::config::ParseOptions::new()).ok()?;
    let id3 = mpeg.id3v2()?;
    for frame in id3 {
        let Frame::Binary(binary) = frame else {
            continue;
        };
        if binary.id().as_str() != "SYLT" {
            continue;
        }
        let Ok(sylt) = SynchronizedTextFrame::parse(&binary.data, FrameFlags::default()) else {
            continue;
        };
        if sylt.timestamp_format != TimestampFormat::MS {
            continue;
        }
        let timed = sylt
            .content
            .into_iter()
            .map(|(ms, text)| (u64::from(ms), text))
            .collect();
        if let Some(l) = Lyrics::synced(LyricsSource::Embedded, timed) {
            return Some(l);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tests::fixture_copy;
    use lofty::config::WriteOptions;
    use lofty::id3::v2::{BinaryFrame, FrameId, SyncTextContentType};
    use lofty::prelude::TagExt;
    use lofty::tag::{Tag, TagType};
    use lofty::TextEncoding;

    fn write_text_lyrics(path: &Path, tag_type: TagType, key: ItemKey, text: &str) {
        let tagged = lofty::read_from_path(path).unwrap();
        let mut tag = tagged
            .tag(tag_type)
            .cloned()
            .unwrap_or_else(|| Tag::new(tag_type));
        assert!(tag.insert_text(key, text.to_string()));
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }

    fn texts(l: &Lyrics) -> Vec<&str> {
        l.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn no_lyrics_anywhere_is_empty() {
        let (_dir, path) = fixture_copy("tagged.mp3", "song.mp3");
        assert_eq!(read(&path), LocalLyrics::default());
    }

    #[test]
    fn missing_and_broken_files_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&dir.path().join("gone.mp3")), LocalLyrics::default());
        let (_d, flac) = fixture_copy("corrupt.flac", "bad.flac");
        assert_eq!(read(&flac), LocalLyrics::default());
        let junk = dir.path().join("junk.mp3");
        std::fs::write(&junk, vec![0xFFu8; 4096]).unwrap();
        assert_eq!(read(&junk), LocalLyrics::default());
    }

    #[test]
    fn sidecar_synced_in_any_extension_case() {
        for ext in ["lrc", "LRC", "Lrc"] {
            let (dir, path) = fixture_copy("tagged.mp3", "Song.mp3");
            std::fs::write(
                dir.path().join(format!("Song.{ext}")),
                "[ar:Testband]\n[00:01.00]Eins\n[00:02.00]Zwei\n",
            )
            .unwrap();
            let got = read(&path);
            let synced = got
                .synced
                .unwrap_or_else(|| panic!("no sidecar for .{ext}"));
            assert_eq!(synced.source, LyricsSource::LrcFile);
            assert_eq!(texts(&synced), ["Eins", "Zwei"]);
            assert_eq!(got.plain, None);
        }
    }

    #[test]
    fn sidecar_with_other_stem_is_ignored() {
        let (dir, path) = fixture_copy("tagged.mp3", "Song.mp3");
        std::fs::write(dir.path().join("Other.lrc"), "[00:01.00]x").unwrap();
        assert_eq!(read(&path), LocalLyrics::default());
    }

    #[test]
    fn sidecar_without_time_tags_is_plain() {
        let (dir, path) = fixture_copy("tagged.mp3", "song.mp3");
        std::fs::write(dir.path().join("song.lrc"), "Erste\nZweite\n").unwrap();
        let got = read(&path);
        assert_eq!(got.synced, None);
        assert_eq!(texts(&got.plain.unwrap()), ["Erste", "Zweite"]);
    }

    #[test]
    fn sidecar_encodings() {
        let (dir, path) = fixture_copy("tagged.mp3", "song.mp3");
        let lrc = dir.path().join("song.lrc");

        let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
        for u in "[00:01.00]Grüße\n".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&lrc, utf16).unwrap();
        assert_eq!(texts(&read(&path).synced.unwrap()), ["Grüße"]);

        let mut utf16be: Vec<u8> = vec![0xFE, 0xFF];
        for u in "[00:01.00]Grüße\n".encode_utf16() {
            utf16be.extend_from_slice(&u.to_be_bytes());
        }
        std::fs::write(&lrc, utf16be).unwrap();
        assert_eq!(texts(&read(&path).synced.unwrap()), ["Grüße"]);

        std::fs::write(&lrc, b"\xEF\xBB\xBF[00:01.00]Bom\n").unwrap();
        assert_eq!(texts(&read(&path).synced.unwrap()), ["Bom"]);

        // Invalid UTF-8 is replaced, not rejected.
        std::fs::write(&lrc, b"[00:01.00]Gr\xFC\xDFe\n").unwrap();
        let lossy = read(&path).synced.unwrap();
        assert!(lossy.lines[0].text.starts_with("Gr"));
    }

    #[test]
    fn oversized_sidecar_is_ignored() {
        let (dir, path) = fixture_copy("tagged.mp3", "song.mp3");
        let line = "[00:01.00]x\n";
        let big = line.repeat(MAX_SIDECAR_BYTES as usize / line.len() + 2);
        std::fs::write(dir.path().join("song.lrc"), big).unwrap();
        assert_eq!(read(&path), LocalLyrics::default());
    }

    #[test]
    fn embedded_plain_lyrics_in_mp3_flac_and_m4a() {
        let cases = [
            ("tagged.mp3", "a.mp3", TagType::Id3v2, ItemKey::UnsyncLyrics),
            (
                "tagged.flac",
                "a.flac",
                TagType::VorbisComments,
                ItemKey::UnsyncLyrics,
            ),
            ("aac.m4a", "a.m4a", TagType::Mp4Ilst, ItemKey::Lyrics),
        ];
        for (fixture, name, tag_type, key) in cases {
            let (_dir, path) = fixture_copy(fixture, name);
            write_text_lyrics(&path, tag_type, key, "Erste Zeile\nZweite Zeile");
            let got = read(&path);
            assert_eq!(got.synced, None, "{name}");
            let plain = got.plain.unwrap_or_else(|| panic!("no lyrics in {name}"));
            assert_eq!(plain.source, LyricsSource::Embedded);
            assert_eq!(texts(&plain), ["Erste Zeile", "Zweite Zeile"], "{name}");
        }
    }

    #[test]
    fn embedded_lrc_text_counts_as_synced() {
        let (_dir, path) = fixture_copy("tagged.flac", "a.flac");
        write_text_lyrics(
            &path,
            TagType::VorbisComments,
            ItemKey::UnsyncLyrics,
            "[00:01.00]Eins\n[00:03.50]Zwei",
        );
        let got = read(&path);
        let synced = got.synced.unwrap();
        assert_eq!(synced.source, LyricsSource::Embedded);
        assert_eq!(synced.lines[1].start_ms, Some(3_500));
        assert_eq!(got.plain, None);
    }

    #[test]
    fn sidecar_wins_over_embedded_per_kind() {
        let (dir, path) = fixture_copy("tagged.mp3", "song.mp3");
        write_text_lyrics(
            &path,
            TagType::Id3v2,
            ItemKey::UnsyncLyrics,
            "[00:01.00]Eingebettet",
        );
        std::fs::write(dir.path().join("song.lrc"), "[00:01.00]Datei").unwrap();
        let got = read(&path);
        assert_eq!(texts(&got.synced.unwrap()), ["Datei"]);
    }

    #[test]
    fn id3_sylt_in_milliseconds_is_synced() {
        let (_dir, path) = fixture_copy("tagged.mp3", "a.mp3");
        write_sylt(&path, TimestampFormat::MS);
        let synced = read(&path).synced.unwrap();
        assert_eq!(synced.source, LyricsSource::Embedded);
        assert_eq!(texts(&synced), ["Eins", "Zwei"]);
        assert_eq!(synced.lines[1].start_ms, Some(2_500));
    }

    #[test]
    fn id3_sylt_in_mpeg_frames_is_skipped() {
        let (_dir, path) = fixture_copy("tagged.mp3", "a.mp3");
        write_sylt(&path, TimestampFormat::MPEG);
        assert_eq!(read(&path), LocalLyrics::default());
    }

    fn write_sylt(path: &Path, format: TimestampFormat) {
        let mut mpeg = MpegFile::read_from(
            &mut File::open(path).unwrap(),
            lofty::config::ParseOptions::new(),
        )
        .unwrap();
        let sylt = SynchronizedTextFrame::new(
            TextEncoding::UTF8,
            *b"deu",
            format,
            SyncTextContentType::Lyrics,
            None,
            vec![(1_000, "Eins".into()), (2_500, "Zwei".into())],
        );
        let data = sylt.as_bytes(WriteOptions::default()).unwrap();
        let frame = BinaryFrame::new(FrameId::Valid("SYLT".into()), data);
        let id3 = mpeg.id3v2_mut().expect("fixture has an ID3v2 tag");
        id3.insert(Frame::Binary(frame));
        mpeg.save_to_path(path, WriteOptions::default()).unwrap();
    }
}
