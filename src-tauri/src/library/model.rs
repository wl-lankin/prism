use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Local,
    Navidrome,
    Radio,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Local => "local",
            Source::Navidrome => "navidrome",
            Source::Radio => "radio",
        }
    }

    pub fn from_db(value: &str) -> Source {
        match value {
            "navidrome" => Source::Navidrome,
            "radio" => Source::Radio,
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

/// An artist of one source. `id` is the name for local artists (what M2 favorites store) and
/// `navidrome:<artist id>` for Navidrome artists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub source: Source,
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
/// (Windows, macOS, as `library::comparable` does), the identity elsewhere. Accepted
/// consequence: `a.flac` and `A.flac` in one folder of a case-sensitive macOS volume map to
/// one id.
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

/// Prefix of every Navidrome track, album and artist id.
pub const NAVIDROME_PREFIX: &str = "navidrome:";

/// Prefix of every Navidrome cover id.
pub const NAVIDROME_COVER_PREFIX: &str = "nd-";

/// Longest server cover id that becomes a Prism cover id.
const NAVIDROME_COVER_MAX: usize = 128;

/// The Prism id of a Navidrome song, album or artist id.
pub fn navidrome_id(raw: &str) -> String {
    format!("{NAVIDROME_PREFIX}{raw}")
}

/// The server id inside a Navidrome id; `None` for any other id or an empty server id.
pub fn navidrome_raw(id: &str) -> Option<&str> {
    id.strip_prefix(NAVIDROME_PREFIX)
        .filter(|raw| !raw.is_empty())
}

fn safe_cover_art(raw: &str) -> bool {
    (1..=NAVIDROME_COVER_MAX).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The cover id of a server `coverArt` value. Cover ids become file names and URL paths, so
/// only `[A-Za-z0-9_.-]{1,128}` is accepted; anything else has no cover.
pub fn navidrome_cover_id(cover_art: &str) -> Option<String> {
    safe_cover_art(cover_art).then(|| format!("{NAVIDROME_COVER_PREFIX}{cover_art}"))
}

/// The server `coverArt` value inside a Navidrome cover id, checked like `navidrome_cover_id`.
pub fn navidrome_cover_raw(id: &str) -> Option<&str> {
    id.strip_prefix(NAVIDROME_COVER_PREFIX)
        .filter(|raw| safe_cover_art(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_radio_round_trips() {
        assert_eq!(Source::Radio.as_str(), "radio");
        assert_eq!(Source::from_db("radio"), Source::Radio);
        assert_eq!(Source::from_db("navidrome"), Source::Navidrome);
        assert_eq!(Source::from_db("local"), Source::Local);
        assert_eq!(serde_json::to_string(&Source::Radio).unwrap(), "\"radio\"");
        assert_eq!(
            serde_json::from_str::<Source>("\"radio\"").unwrap(),
            Source::Radio
        );
    }

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
    fn navidrome_ids_round_trip() {
        let id = navidrome_id("so-42");
        assert_eq!(id, "navidrome:so-42");
        assert_eq!(navidrome_raw(&id), Some("so-42"));
        assert_eq!(navidrome_raw("local:abc"), None);
        assert_eq!(navidrome_raw("Massive Attack"), None);
        assert_eq!(navidrome_raw("navidrome:"), None);
        let cover = navidrome_cover_id("al-1_abc").unwrap();
        assert_eq!(cover, "nd-al-1_abc");
        assert_eq!(navidrome_cover_raw(&cover), Some("al-1_abc"));
        assert_eq!(navidrome_cover_raw("abcdef"), None);
        assert_eq!(navidrome_cover_raw("nd-../x"), None);
    }

    #[test]
    fn cover_id_rejects_unsafe_chars() {
        assert!(navidrome_cover_id("al-1_abc").is_some());
        assert!(navidrome_cover_id("mf-2.jpg").is_some());
        assert!(navidrome_cover_id(&"a".repeat(128)).is_some());
        assert_eq!(navidrome_cover_id("../x"), None);
        assert_eq!(navidrome_cover_id("a b"), None);
        assert_eq!(navidrome_cover_id("a/b"), None);
        assert_eq!(navidrome_cover_id("a\\b"), None);
        assert_eq!(navidrome_cover_id("ä"), None);
        assert_eq!(navidrome_cover_id(""), None);
        assert_eq!(navidrome_cover_id(&"a".repeat(129)), None);
    }

    #[test]
    fn artist_serializes_id_and_source() {
        let artist = Artist {
            id: "navidrome:ar-1".into(),
            source: Source::Navidrome,
            name: "Lumen Field".into(),
            album_count: 1,
            track_count: 3,
            cover_id: None,
        };
        let value = serde_json::to_value(&artist).unwrap();
        assert_eq!(value["id"], "navidrome:ar-1");
        assert_eq!(value["source"], "navidrome");
        assert_eq!(value["albumCount"], 1);
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
