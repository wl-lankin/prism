use std::path::{Path, PathBuf};

use anyhow::Context;
use sha1::{Digest, Sha1};

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "png", "webp", "gif", "bmp"];
const FOLDER_COVER_NAMES: &[&str] = &["cover", "folder", "front", "album"];
const FOLDER_COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Content-addressed cover images on disk: `<dir>/<sha1>.<ext>`.
pub struct CoverStore {
    dir: PathBuf,
}

impl CoverStore {
    pub fn new(dir: PathBuf) -> CoverStore {
        CoverStore { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Stores an image and returns its id (the sha1 of the bytes). Identical bytes are
    /// written once.
    pub fn store_bytes(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let ext = image_extension(bytes).context("unrecognised cover image format")?;
        let id = hex::encode(Sha1::digest(bytes));
        let target = self.dir.join(format!("{id}.{ext}"));
        if target.exists() {
            return Ok(id);
        }
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating cover dir {}", self.dir.display()))?;
        // Write to a unique temp name, then rename: parallel scans never see half a file.
        let temp = self.dir.join(format!(
            "{id}.{}.{:?}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&temp, bytes)
            .with_context(|| format!("writing cover {}", temp.display()))?;
        if let Err(err) = std::fs::rename(&temp, &target) {
            let _ = std::fs::remove_file(&temp);
            if !target.exists() {
                return Err(err).with_context(|| format!("storing cover {}", target.display()));
            }
        }
        Ok(id)
    }

    pub fn path(&self, id: &str) -> Option<PathBuf> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        IMAGE_EXTENSIONS
            .iter()
            .map(|ext| self.dir.join(format!("{id}.{ext}")))
            .find(|path| path.is_file())
    }
}

/// Detects the image type from its magic bytes.
pub fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"BM") && bytes.len() > 14 {
        Some("bmp")
    } else {
        None
    }
}

/// The priority of `path` as a folder cover by its file name (0 is best), or `None` when the
/// name is not one `find_folder_cover` accepts.
pub fn folder_cover_rank(path: &Path) -> Option<usize> {
    let stem = path.file_stem()?.to_string_lossy().to_lowercase();
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if !FOLDER_COVER_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    FOLDER_COVER_NAMES.iter().position(|name| *name == stem)
}

/// Finds a cover image next to the audio files: `cover`, `folder`, `front` or `album`
/// with a jpg, jpeg, png or webp extension, case-insensitive. Earlier names win.
pub fn find_folder_cover(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let Some(rank) = folder_cover_rank(&path) else {
            continue;
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let better = match &best {
            None => true,
            Some((best_rank, best_path)) => {
                rank < *best_rank || (rank == *best_rank && path < *best_path)
            }
        };
        if better {
            best = Some((rank, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];

    #[test]
    fn stores_once_by_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = CoverStore::new(dir.path().join("covers"));
        let id = store.store_bytes(PNG).unwrap();
        assert_eq!(id, hex::encode(Sha1::digest(PNG)));
        assert_eq!(store.store_bytes(PNG).unwrap(), id);
        let path = store.path(&id).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{id}.png")
        );
        assert_eq!(std::fs::read_dir(store.dir()).unwrap().count(), 1);
        assert!(store.path("deadbeef").is_none());
        assert!(store.path("../x").is_none());
        assert!(store.store_bytes(b"not an image").is_err());
    }

    #[test]
    fn finds_folder_cover_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_folder_cover(dir.path()).is_none());
        std::fs::write(dir.path().join("Album.PNG"), PNG).unwrap();
        std::fs::write(dir.path().join("notes.jpg"), PNG).unwrap();
        assert_eq!(
            find_folder_cover(dir.path()).unwrap().file_name().unwrap(),
            "Album.PNG"
        );
        std::fs::write(dir.path().join("Cover.JPEG"), PNG).unwrap();
        assert_eq!(
            find_folder_cover(dir.path()).unwrap().file_name().unwrap(),
            "Cover.JPEG"
        );
    }
}
