//! Locates the external ffmpeg tools. Lookup order: the `PRISM_FFPROBE` / `PRISM_FFMPEG`
//! environment variable, then a sidecar next to the executable, then `PATH`.
//!
//! Release builds bundle the tools as Tauri sidecars (`src-tauri/tauri.sidecar.conf.json`),
//! which Tauri installs next to the executable without the target triple: the install folder on
//! Windows, `Prism.app/Contents/MacOS` on macOS. Dev builds bundle nothing and use `PATH`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const FFMPEG: &str = "ffmpeg";
const FFPROBE: &str = "ffprobe";

pub fn ffprobe_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| resolve("PRISM_FFPROBE", FFPROBE))
        .clone()
}

pub fn ffmpeg_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| resolve("PRISM_FFMPEG", FFMPEG)).clone()
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
    resolve_from(std::env::var_os(env_var), name)
}

/// `resolve` with the value of the environment variable passed in (so tests do not touch the
/// process environment).
fn resolve_from(env_value: Option<OsString>, name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    resolve_in(env_value, exe.as_deref(), std::env::var_os("PATH"), name)
}

/// The lookup with the environment variable's value, the running executable and `PATH` passed
/// in.
fn resolve_in(
    env_value: Option<OsString>,
    exe: Option<&Path>,
    path_var: Option<OsString>,
    name: &str,
) -> Option<PathBuf> {
    if let Some(path) = env_value.map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let file_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Some(sidecar) = exe.and_then(|exe| sidecar_in(exe, &file_name)) {
        return Some(sidecar);
    }
    std::env::split_paths(&path_var?)
        .map(|dir| dir.join(&file_name))
        .find(|candidate| candidate.is_file())
}

/// The sidecar `file_name` next to `exe`, or next to the file `exe` links to (Prism started
/// through a link sees the link's folder as its own).
fn sidecar_in(exe: &Path, file_name: &str) -> Option<PathBuf> {
    let beside = |exe: &Path| exe.parent().map(|dir| dir.join(file_name));
    beside(exe).filter(|path| path.is_file()).or_else(|| {
        let real = exe.canonicalize().ok()?;
        beside(&real).filter(|path| path.is_file())
    })
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
            // `-vn` as Prism passes it: the bundled build has no decoder for embedded cover art.
            .args(["-vn", "-f", "null", "-"])
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
        assert_eq!(
            resolve_from(Some(fake.clone().into_os_string()), "prism-no-such-tool"),
            Some(fake)
        );
        assert_eq!(resolve_from(None, "prism-no-such-tool"), None);
        let missing = dir.path().join("missing-probe").into_os_string();
        assert_eq!(resolve_from(Some(missing), "prism-no-such-tool"), None);
    }

    fn touch(path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
        path.to_path_buf()
    }

    fn tool_file(name: &str) -> String {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    }

    #[test]
    fn finds_the_sidecar_in_the_mac_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Prism.app/Contents/MacOS");
        let exe = touch(&macos.join("prism"));
        let sidecar = touch(&macos.join(tool_file(FFMPEG)));
        assert_eq!(resolve_in(None, Some(&exe), None, FFMPEG), Some(sidecar));
        assert_eq!(resolve_in(None, Some(&exe), None, FFPROBE), None);
    }

    #[test]
    fn finds_the_sidecar_in_the_install_folder() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Programs").join("Prism");
        let exe = touch(&install.join(tool_file("prism")));
        let sidecar = touch(&install.join(tool_file(FFPROBE)));
        assert_eq!(resolve_in(None, Some(&exe), None, FFPROBE), Some(sidecar));
    }

    #[test]
    fn lookup_order_is_variable_then_sidecar_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = touch(&dir.path().join("app").join(tool_file("prism")));
        let on_path = dir.path().join("bin");
        let path_tool = touch(&on_path.join(tool_file(FFMPEG)));
        let path_var = Some(std::env::join_paths([&on_path]).unwrap());
        assert_eq!(
            resolve_in(None, Some(&exe), path_var.clone(), FFMPEG),
            Some(path_tool)
        );
        let sidecar = touch(&dir.path().join("app").join(tool_file(FFMPEG)));
        assert_eq!(
            resolve_in(None, Some(&exe), path_var.clone(), FFMPEG),
            Some(sidecar)
        );
        let chosen = touch(&dir.path().join("chosen-ffmpeg"));
        assert_eq!(
            resolve_in(
                Some(chosen.clone().into_os_string()),
                Some(&exe),
                path_var,
                FFMPEG
            ),
            Some(chosen)
        );
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_sidecar_next_to_a_linked_executable() {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Prism.app/Contents/MacOS");
        let exe = touch(&macos.join("prism"));
        let sidecar = touch(&macos.join(FFMPEG));
        let link = dir.path().join("bin").join("prism");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let found = resolve_in(None, Some(&link), None, FFMPEG).unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            sidecar.canonicalize().unwrap()
        );
    }

    /// The release config bundles exactly the tools this module looks up, under the names Tauri
    /// installs next to the executable; the base config bundles nothing, so dev builds use PATH.
    #[test]
    fn the_sidecar_config_bundles_what_the_lookup_finds() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let read = |name: &str| -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(manifest.join(name)).unwrap()).unwrap()
        };
        let sidecar = read("tauri.sidecar.conf.json");
        let bins: Vec<&str> = sidecar["bundle"]["externalBin"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bin| bin.as_str().unwrap())
            .collect();
        assert_eq!(
            bins,
            [format!("binaries/{FFMPEG}"), format!("binaries/{FFPROBE}")]
        );
        assert!(read("tauri.conf.json")["bundle"]
            .get("externalBin")
            .is_none());
    }
}
