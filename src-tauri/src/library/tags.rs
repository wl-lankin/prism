use std::fs::File;
use std::path::Path;

use anyhow::Context;
use lofty::file::TaggedFile;
use lofty::picture::PictureType;
use lofty::prelude::{Accessor, AudioFile, ItemKey, TaggedFileExt};
use lofty::tag::Tag;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use super::model::{album_id, track_id, Source, Track};

pub const UNKNOWN_ARTIST: &str = "Unknown artist";
pub const UNKNOWN_ALBUM: &str = "Unknown album";

/// Containers neither lofty nor symphonia can parse. They are read with ffprobe; without
/// ffprobe they are indexed from the file name with duration 0 (and re-read on every scan).
pub(crate) const UNPARSED_EXTENSIONS: &[&str] = &["wma", "tta", "dsf", "dff"];

/// Everything read from one audio file: the Track fields plus the raw embedded cover.
#[derive(Debug, Clone)]
pub struct TrackMeta {
    pub track: Track,
    pub embedded_cover: Option<Vec<u8>>,
}

/// Reads tags and stream properties with lofty, falling back to a symphonia probe for
/// duration and format when lofty cannot read the file, and to ffprobe for the containers
/// neither of them parses.
pub fn read(path: &Path) -> anyhow::Result<TrackMeta> {
    let extension = extension_of(path);
    let lofty_err = match lofty::read_from_path(path) {
        Ok(file) => return Ok(from_lofty(path, &extension, &file)),
        Err(err) => err,
    };
    if UNPARSED_EXTENSIONS.contains(&extension.as_str()) {
        return Ok(match ffprobe(path) {
            Some(fields) => build(path, &extension, fields, None),
            None => minimal(path, &extension),
        });
    }
    probe(path, &extension).map_err(|probe_err| {
        anyhow::anyhow!("unreadable audio file: {lofty_err}; probe: {probe_err:#}")
    })
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn non_empty(value: Option<impl AsRef<str>>) -> Option<String> {
    value
        .map(|v| v.as_ref().trim().to_string())
        .filter(|v| !v.is_empty())
}

fn minimal(path: &Path, extension: &str) -> TrackMeta {
    build(path, extension, Fields::default(), None)
}

#[derive(Default)]
struct Fields {
    title: Option<String>,
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    duration_ms: u64,
    year: Option<i32>,
    genre: Option<String>,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    bitrate_kbps: Option<u32>,
    replay_gain_track_db: Option<f32>,
    replay_gain_album_db: Option<f32>,
    replay_gain_track_peak: Option<f32>,
    replay_gain_album_peak: Option<f32>,
}

fn build(path: &Path, extension: &str, f: Fields, embedded_cover: Option<Vec<u8>>) -> TrackMeta {
    let title = f.title.unwrap_or_else(|| stem_of(path));
    let artist = f.artist.unwrap_or_else(|| UNKNOWN_ARTIST.to_string());
    let album_artist = f.album_artist.unwrap_or_else(|| artist.clone());
    let album = f.album.unwrap_or_else(|| UNKNOWN_ALBUM.to_string());
    let track = Track {
        id: track_id(path),
        source: Source::Local,
        album_id: album_id(&album_artist, &album),
        title,
        artist,
        album_artist,
        album,
        track_no: f.track_no,
        disc_no: f.disc_no,
        duration_ms: f.duration_ms,
        year: f.year,
        genre: f.genre,
        format: extension.to_string(),
        sample_rate: f.sample_rate,
        bit_depth: f.bit_depth,
        bitrate_kbps: f.bitrate_kbps,
        cover_id: None,
        path: Some(path.to_string_lossy().into_owned()),
        replay_gain_track_db: f.replay_gain_track_db,
        replay_gain_album_db: f.replay_gain_album_db,
        replay_gain_track_peak: f.replay_gain_track_peak,
        replay_gain_album_peak: f.replay_gain_album_peak,
    };
    TrackMeta {
        track,
        embedded_cover,
    }
}

/// Parses "-6.50 dB" or "0.891250" style ReplayGain values.
fn parse_gain(value: Option<&str>) -> Option<f32> {
    let text = value?.trim();
    let number = text
        .strip_suffix("dB")
        .or_else(|| text.strip_suffix("db"))
        .or_else(|| text.strip_suffix("DB"))
        .unwrap_or(text)
        .trim();
    number.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn tag_value(tags: &[&Tag], key: ItemKey) -> Option<String> {
    tags.iter().find_map(|tag| non_empty(tag.get_string(key)))
}

fn from_lofty(path: &Path, extension: &str, file: &TaggedFile) -> TrackMeta {
    // Primary tag first, then any other tag the file carries (e.g. ID3v1 next to ID3v2).
    let mut tags: Vec<&Tag> = Vec::new();
    if let Some(primary) = file.primary_tag() {
        tags.push(primary);
    }
    for tag in file.tags() {
        if !tags.iter().any(|t| std::ptr::eq(*t, tag)) {
            tags.push(tag);
        }
    }

    let first = |get: &dyn Fn(&Tag) -> Option<String>| tags.iter().find_map(|tag| get(tag));
    let first_u32 = |get: &dyn Fn(&Tag) -> Option<u32>| tags.iter().find_map(|tag| get(tag));

    let properties = file.properties();
    let fields = Fields {
        title: first(&|t| non_empty(t.title())),
        artist: first(&|t| non_empty(t.artist())),
        album_artist: tag_value(&tags, ItemKey::AlbumArtist),
        album: first(&|t| non_empty(t.album())),
        track_no: first_u32(&|t| t.track()),
        disc_no: first_u32(&|t| t.disk()),
        duration_ms: properties.duration().as_millis() as u64,
        year: tags
            .iter()
            .find_map(|t| t.date())
            .map(|date| i32::from(date.year))
            .filter(|year| *year > 0),
        genre: first(&|t| non_empty(t.genre())),
        sample_rate: properties.sample_rate(),
        bit_depth: properties.bit_depth(),
        bitrate_kbps: properties
            .audio_bitrate()
            .or_else(|| properties.overall_bitrate())
            .filter(|kbps| *kbps > 0),
        replay_gain_track_db: parse_gain(tag_value(&tags, ItemKey::ReplayGainTrackGain).as_deref()),
        replay_gain_album_db: parse_gain(tag_value(&tags, ItemKey::ReplayGainAlbumGain).as_deref()),
        replay_gain_track_peak: parse_gain(
            tag_value(&tags, ItemKey::ReplayGainTrackPeak).as_deref(),
        ),
        replay_gain_album_peak: parse_gain(
            tag_value(&tags, ItemKey::ReplayGainAlbumPeak).as_deref(),
        ),
    };

    // Front cover first, otherwise whatever picture the file carries.
    let pictures = tags.iter().flat_map(|tag| tag.pictures());
    let cover = pictures
        .clone()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.clone().next())
        .map(|p| p.data().to_vec())
        .filter(|data| !data.is_empty());

    build(path, extension, fields, cover)
}

/// Tags and duration from `ffprobe`, or `None` when ffprobe is missing or fails.
fn ffprobe(path: &Path) -> Option<Fields> {
    let program = crate::tools::ffprobe_path()?;
    let output = crate::tools::command(&program)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:format_tags",
            "-of",
            "json",
            "-i",
        ])
        .arg(crate::tools::file_input(path))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    fields_from_ffprobe(&String::from_utf8_lossy(&output.stdout))
}

/// Leading number of "4" or "4/12".
fn leading_number(value: &str) -> Option<u32> {
    let digits: String = value
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok().filter(|n| *n > 0)
}

fn fields_from_ffprobe(json: &str) -> Option<Fields> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let format = value.get("format")?;
    let tags: std::collections::HashMap<String, String> = format
        .get("tags")
        .and_then(|tags| tags.as_object())
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| Some((key.to_lowercase(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let tag = |keys: &[&str]| keys.iter().find_map(|key| non_empty(tags.get(*key)));
    let duration_ms = format
        .get("duration")
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| (d * 1000.0).round() as u64)
        .unwrap_or(0);
    Some(Fields {
        title: tag(&["title"]),
        artist: tag(&["artist"]),
        album_artist: tag(&["album_artist", "albumartist", "wm/albumartist"]),
        album: tag(&["album"]),
        track_no: tag(&["track", "tracknumber", "wm/tracknumber"]).and_then(|t| leading_number(&t)),
        disc_no: tag(&["disc", "discnumber", "wm/partofset"]).and_then(|d| leading_number(&d)),
        year: tag(&["date", "year", "wm/year"])
            .and_then(|d| leading_number(&d.chars().take(4).collect::<String>()))
            .map(|y| y as i32),
        genre: tag(&["genre"]),
        duration_ms,
        ..Fields::default()
    })
}

fn probe(path: &Path, extension: &str) -> anyhow::Result<TrackMeta> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if !extension.is_empty() {
        hint.with_extension(extension);
    }
    let reader = symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = reader
        .default_track(TrackType::Audio)
        .context("no audio track")?;
    let duration_ms = match (track.time_base, track.duration) {
        (Some(base), Some(duration)) => base
            .calc_duration(duration)
            .map(|time| time.as_millis().max(0) as u64)
            .unwrap_or(0),
        _ => 0,
    };
    let (sample_rate, bit_depth) = match &track.codec_params {
        Some(CodecParameters::Audio(audio)) => (
            audio.sample_rate,
            audio
                .bits_per_sample
                .and_then(|bits| u8::try_from(bits).ok()),
        ),
        _ => (None, None),
    };
    let fields = Fields {
        duration_ms,
        sample_rate,
        bit_depth,
        ..Fields::default()
    };
    Ok(build(path, extension, fields, None))
}

#[cfg(test)]
pub(crate) fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_basic_tags() {
        let meta = read(&fixtures().join("tagged.mp3")).unwrap();
        let track = meta.track;
        assert_eq!(track.title, "Alpha");
        assert_eq!(track.artist, "Fixture Artist");
        assert_eq!(track.album, "Fixture Album");
        assert_eq!(track.track_no, Some(1));
        assert!(
            (900..=1100).contains(&track.duration_ms),
            "duration {}",
            track.duration_ms
        );
        assert_eq!(track.format, "mp3");
    }

    #[test]
    fn missing_tags_fall_back() {
        let track = read(&fixtures().join("notags.ogg")).unwrap().track;
        assert_eq!(track.title, "notags");
        assert_eq!(track.album, "Unknown album");
        assert_eq!(track.artist, "Unknown artist");
        assert_eq!(track.album_artist, "Unknown artist");
        assert!(track.duration_ms > 900);
    }

    #[test]
    fn latin1_tag_decodes() {
        let track = read(&fixtures().join("latin1.mp3")).unwrap().track;
        assert_eq!(track.title, "Caf\u{e9}");
    }

    #[test]
    fn reads_replaygain_tags() {
        let track = read(&fixtures().join("tagged.flac")).unwrap().track;
        assert_eq!(track.replay_gain_track_db, Some(-6.5));
        assert_eq!(track.replay_gain_album_db, Some(-7.25));
        assert_eq!(track.replay_gain_track_peak, Some(0.89125));
        assert_eq!(track.replay_gain_album_peak, Some(0.977));
    }

    #[test]
    fn parses_gain_spellings() {
        assert_eq!(parse_gain(Some("+1.20 dB")), Some(1.2));
        assert_eq!(parse_gain(Some("-3 db")), Some(-3.0));
        assert_eq!(parse_gain(Some("0.5")), Some(0.5));
        assert_eq!(parse_gain(Some("loud")), None);
        assert_eq!(parse_gain(None), None);
    }

    #[test]
    fn embedded_cover_is_extracted() {
        let meta = read(&fixtures().join("tagged.flac")).unwrap();
        let cover = meta.embedded_cover.expect("embedded cover");
        assert!(cover.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(read(&fixtures().join("tagged.mp3"))
            .unwrap()
            .embedded_cover
            .is_none());
    }

    #[test]
    fn wma_tags_via_ffprobe() {
        if crate::tools::ffprobe_path().is_none() {
            println!("ffprobe not found: skipping wma_tags_via_ffprobe");
            return;
        }
        let track = read(&fixtures().join("wma.wma")).unwrap().track;
        assert_eq!(track.title, "Wma");
        assert_eq!(track.artist, "Fixture Artist");
        assert_eq!(track.album, "Probe Album");
        assert_eq!(track.track_no, Some(3));
        assert_eq!(track.year, Some(2024));
        assert_eq!(track.format, "wma");
        assert!(
            (900..=1100).contains(&track.duration_ms),
            "duration {}",
            track.duration_ms
        );
        let tta = read(&fixtures().join("tta.tta")).unwrap().track;
        assert!(
            (900..=1100).contains(&tta.duration_ms),
            "tta {}",
            tta.duration_ms
        );
    }

    #[test]
    fn ffprobe_tags_of_a_dash_named_file_in_a_folder_with_spaces() {
        if crate::tools::ffprobe_path().is_none() {
            println!("ffprobe not found: skipping the dash-named ffprobe test");
            return;
        }
        let (_dir, path) = crate::tools::tests::fixture_copy("wma.wma", "-x.wma");
        let track = read(&path).unwrap().track;
        assert_eq!(track.title, "Wma");
        assert!(
            (900..=1100).contains(&track.duration_ms),
            "{}",
            track.duration_ms
        );
    }

    #[test]
    fn maps_ffprobe_json() {
        let json = r#"{"format":{"duration":"61.500000","tags":{"TITLE":"T","Artist":"A",
            "album":"Al","album_artist":"AA","track":"4/12","disc":"2/2","date":"1999-05-01",
            "genre":"Jazz"}}}"#;
        let fields = fields_from_ffprobe(json).unwrap();
        assert_eq!(fields.title.as_deref(), Some("T"));
        assert_eq!(fields.artist.as_deref(), Some("A"));
        assert_eq!(fields.album.as_deref(), Some("Al"));
        assert_eq!(fields.album_artist.as_deref(), Some("AA"));
        assert_eq!(fields.track_no, Some(4));
        assert_eq!(fields.disc_no, Some(2));
        assert_eq!(fields.year, Some(1999));
        assert_eq!(fields.genre.as_deref(), Some("Jazz"));
        assert_eq!(fields.duration_ms, 61_500);
    }

    #[test]
    fn reads_every_fixture_format() {
        for name in ["opus.opus", "wma.wma", "wv.wv", "tta.tta", "aac.m4a"] {
            let track = read(&fixtures().join(name))
                .unwrap_or_else(|err| panic!("{name}: {err:#}"))
                .track;
            assert!(!track.title.is_empty(), "{name}");
        }
    }
}
