//! Locates the external ffmpeg tools. Lookup order: the `PRISM_FFPROBE` / `PRISM_FFMPEG`
//! environment variable, then a sidecar next to the executable, then `PATH`.

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
mod tests {
    use super::*;

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
