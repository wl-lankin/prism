//! Locates the external ffmpeg tools. Lookup order: the `PRISM_FFPROBE` / `PRISM_FFMPEG`
//! environment variable, then a sidecar next to the executable, then `PATH`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub fn ffprobe_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| resolve("PRISM_FFPROBE", "ffprobe"))
        .clone()
}

pub fn ffmpeg_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| resolve("PRISM_FFMPEG", "ffmpeg"))
        .clone()
}

/// A `Command` for a tool that never opens a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A tool input argument for `path`: prefixed with `file:` so that a file name is never read
/// as an option or as a protocol (`http:`, `concat:`, `-x.flac`). Works for Windows paths
/// with a drive letter and spaces.
pub fn file_input(path: &Path) -> OsString {
    let mut input = OsString::from("file:");
    input.push(path.as_os_str());
    input
}

fn resolve(env_var: &str, name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(env_var).map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let file_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let sidecar = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file_name)));
    if let Some(sidecar) = sidecar.filter(|path| path.is_file()) {
        return Some(sidecar);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(&file_name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn file_input_is_prefixed_and_keeps_the_path() {
        assert_eq!(
            file_input(Path::new("-x.flac")),
            OsString::from("file:-x.flac")
        );
        assert_eq!(
            file_input(Path::new(r"C:\My Music\a b.flac")),
            OsString::from(r"file:C:\My Music\a b.flac")
        );
    }

    /// A fixture copied to a temp dir (with a space in its name) under `name`.
    pub(crate) fn fixture_copy(fixture: &str, name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("prism hard ")
            .tempdir()
            .unwrap();
        let path = dir.path().join(name);
        std::fs::copy(crate::library::tags::fixtures().join(fixture), &path).unwrap();
        (dir, path)
    }

    #[test]
    fn ffprobe_reads_a_file_named_like_an_option() {
        let Some(program) = ffprobe_path() else {
            println!("ffprobe not found: skipping ffprobe_reads_a_file_named_like_an_option");
            return;
        };
        let (dir, _path) = fixture_copy("tagged.flac", "-x.flac");
        // A relative name starting with "-" is the dangerous case: run it from its folder.
        let output = command(&program)
            .current_dir(dir.path())
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "json",
                "-i",
            ])
            .arg(file_input(Path::new("-x.flac")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn ffmpeg_opens_a_file_named_like_an_option() {
        let Some(program) = ffmpeg_path() else {
            println!("ffmpeg not found: skipping ffmpeg_opens_a_file_named_like_an_option");
            return;
        };
        let (dir, _path) = fixture_copy("tagged.flac", "-x.flac");
        let output = command(&program)
            .current_dir(dir.path())
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(file_input(Path::new("-x.flac")))
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn resolves_from_the_environment_variable_first() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-probe");
        std::fs::write(&fake, b"").unwrap();
        let var = "PRISM_TEST_TOOL_OVERRIDE";
        std::env::set_var(var, &fake);
        assert_eq!(resolve(var, "prism-no-such-tool"), Some(fake));
        std::env::remove_var(var);
        assert_eq!(resolve(var, "prism-no-such-tool"), None);
    }
}
