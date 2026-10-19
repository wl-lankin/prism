use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Local,
    Navidrome,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Local => "local",
            Source::Navidrome => "navidrome",
        }
    }

    pub fn from_db(value: &str) -> Source {
        match value {
            "navidrome" => Source::Navidrome,
            _ => Source::Local,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub album_id: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_ms: u64,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub format: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub cover_id: Option<String>,
    pub path: Option<String>,
    pub replay_gain_track_db: Option<f32>,
    pub replay_gain_album_db: Option<f32>,
    pub replay_gain_track_peak: Option<f32>,
    pub replay_gain_album_peak: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    pub cover_id: Option<String>,
    pub track_count: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    pub cover_id: Option<String>,
}

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Normalizes a path for id purposes: canonical when possible, otherwise the
/// given path made absolute. On Windows the verbatim prefix is dropped, the
/// drive letter is lowercased and separators are backslashes.
fn normalize_path(path: &Path) -> String {
    let resolved: PathBuf = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    };
    let text = resolved.to_string_lossy().into_owned();
    if cfg!(windows) {
        let text = text
            .strip_prefix(r"\\?\")
            .unwrap_or(&text)
            .replace('/', "\\");
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
                format!("{}{}", letter.to_ascii_lowercase(), &text[1..])
            }
            _ => text,
        }
    } else {
        text
    }
}

pub fn track_id(path: &Path) -> String {
    format!("local:{}", sha1_hex(&normalize_path(path)))
}

pub fn album_id(album_artist: &str, album: &str) -> String {
    let key = format!(
        "{}\u{1f}{}",
        album_artist.to_lowercase(),
        album.to_lowercase()
    );
    format!("local:{}", sha1_hex(&key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_id_is_stable_and_prefixed() {
        let path = Path::new("C:\\Music\\does-not-exist\\a.flac");
        let first = track_id(path);
        assert_eq!(first, track_id(path));
        assert!(first.starts_with("local:"));
        assert_eq!(first.len(), "local:".len() + 40);
        assert_ne!(first, track_id(Path::new("C:\\Music\\does-not-exist\\b.flac")));
    }

    #[cfg(windows)]
    #[test]
    fn track_id_ignores_drive_letter_case_on_windows() {
        assert_eq!(
            track_id(Path::new("C:\\Music\\none\\a.flac")),
            track_id(Path::new("c:\\Music\\none\\a.flac"))
        );
    }

    #[test]
    fn album_id_is_case_insensitive_and_stable() {
        let id = album_id("Radiohead", "Kid A");
        assert_eq!(id, album_id("RADIOHEAD", "kid a"));
        assert!(id.starts_with("local:"));
        assert_ne!(id, album_id("Radiohead", "Amnesiac"));
        assert_ne!(album_id("ab", "c"), album_id("a", "bc"));
    }

    #[test]
    fn track_serializes_camel_case_and_lowercase_source() {
        let track = Track {
            id: "local:x".into(),
            source: Source::Navidrome,
            title: "T".into(),
            artist: "A".into(),
            album_artist: "AA".into(),
            album: "Al".into(),
            album_id: "local:y".into(),
            track_no: Some(1),
            disc_no: None,
            duration_ms: 1234,
            year: None,
            genre: None,
            format: "flac".into(),
            sample_rate: None,
            bit_depth: None,
            bitrate_kbps: None,
            cover_id: None,
            path: None,
            replay_gain_track_db: Some(-6.5),
            replay_gain_album_db: None,
            replay_gain_track_peak: None,
            replay_gain_album_peak: None,
        };
        let value = serde_json::to_value(&track).unwrap();
        assert_eq!(value["source"], "navidrome");
        assert_eq!(value["albumArtist"], "AA");
        assert_eq!(value["durationMs"], 1234);
        assert_eq!(value["trackNo"], 1);
        assert_eq!(value["replayGainTrackDb"], -6.5);
        assert!(value.get("album_artist").is_none());
    }
}
