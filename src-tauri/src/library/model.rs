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

/// A library folder as `library_folders` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub path: String,
    pub online: bool,
    pub track_count: u32,
}

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Case folding for id keys: lowercase where the file system usually ignores case
/// (Windows, macOS, as `library::comparable` does), the identity elsewhere.
fn fold(text: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// The hashed key of a library track: the folded folder key, a unit separator, and the
/// folded relative path with its components joined by `/`. Pure string work: no file
/// system call, so an offline share or a slow NAS never changes or delays an id.
pub(crate) fn id_key(folder_key: &str, relative: &Path) -> String {
    let rel = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("{}\u{1f}{}", fold(folder_key), fold(&rel))
}

/// Normalizes a path for `file:` ids: the given path made absolute (no `canonicalize`, so
/// no file system call). On Windows the verbatim prefix is dropped, the drive letter is
/// lowercased and separators are backslashes.
fn normalize_path(path: &Path) -> String {
    let resolved: PathBuf = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
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

/// Id of an indexed track: `local:<sha1 of id_key(folder_key, relative)>`, where `relative`
/// is the file's path below its library folder. Case-insensitive on Windows and macOS.
pub fn track_id(folder_key: &str, relative: &Path) -> String {
    format!("local:{}", sha1_hex(&id_key(folder_key, relative)))
}

/// Id of a file played without indexing (dropped onto the window): `file:<sha1 of the path>`.
pub fn file_track_id(path: &Path) -> String {
    format!("file:{}", sha1_hex(&normalize_path(path)))
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
    fn track_id_uses_folder_and_relative_path() {
        let rel = Path::new("Artist").join("Album").join("01.flac");
        let id = track_id("C:\\Music", &rel);
        assert_eq!(id, track_id("C:\\Music", &rel));
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), "local:".len() + 40);
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(id, track_id("c:\\MUSIC", &rel));
            assert_eq!(
                id,
                track_id(
                    "C:\\Music",
                    &Path::new("artist").join("ALBUM").join("01.flac")
                )
            );
        } else {
            assert_ne!(id, track_id("c:\\MUSIC", &rel));
        }
        assert_ne!(
            id,
            track_id(
                "C:\\Music",
                &Path::new("Artist").join("Album").join("02.flac")
            )
        );
        assert_ne!(id, track_id("D:\\Music", &rel));
        // The separator keeps folder and relative path apart.
        assert_ne!(
            track_id("ab", Path::new("c")),
            track_id("a", Path::new("bc"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn track_id_joins_components_with_slashes() {
        assert_eq!(
            track_id("C:\\Music", Path::new("Artist\\a.flac")),
            track_id("C:\\Music", Path::new("Artist/a.flac"))
        );
        assert!(id_key("C:\\Music", Path::new("Artist\\a.flac")).ends_with("artist/a.flac"));
    }

    #[test]
    fn track_id_does_not_touch_the_file_system() {
        let folder = r"\\nas\music";
        let rel = Path::new("Some Artist").join("a.flac");
        let key = id_key(folder, &rel);
        assert!(!key.to_lowercase().contains("unc"), "{key}");
        assert_eq!(
            key,
            format!("{}\u{1f}{}", fold(folder), fold("Some Artist/a.flac"))
        );
        let id = track_id(folder, &rel);
        assert_eq!(id, track_id(folder, &rel));
        assert_eq!(id, format!("local:{}", sha1_hex(&key)));
    }

    #[test]
    fn file_track_id_is_stable_and_prefixed() {
        let path = Path::new("C:\\Music\\does-not-exist\\a.flac");
        let first = file_track_id(path);
        assert_eq!(first, file_track_id(path));
        assert!(first.starts_with("file:"));
        assert_eq!(first.len(), "file:".len() + 40);
        assert_ne!(
            first,
            file_track_id(Path::new("C:\\Music\\does-not-exist\\b.flac"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_track_id_ignores_drive_letter_case_and_skips_canonicalize() {
        assert_eq!(
            file_track_id(Path::new("C:\\Music\\none\\a.flac")),
            file_track_id(Path::new("c:\\Music\\none\\a.flac"))
        );
        assert_eq!(
            normalize_path(Path::new(r"\\nas\music\a.flac")),
            r"\\nas\music\a.flac"
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
