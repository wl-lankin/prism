pub mod db;
pub mod library;
pub mod tools;

use std::sync::Arc;

use anyhow::Context;
use tauri::Manager;

use db::Db;
use library::covers::CoverStore;
use library::{tauri_emitter, watch, Library};

/// Shared backend state, managed by Tauri.
pub struct AppState {
    pub library: Arc<Library>,
}

fn setup(app: &mut tauri::App) -> anyhow::Result<()> {
    let data_dir = app
        .path()
        .app_data_dir()
        .context("resolving the app data dir")?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let cover_dir = app
        .path()
        .app_cache_dir()
        .context("resolving the app cache dir")?
        .join("covers")
        .join("original");

    let db = Arc::new(Db::open(&data_dir.join("library.db"))?);
    let library = Arc::new(Library::new(db, Arc::new(CoverStore::new(cover_dir))));
    let folders = library.folders()?;
    app.manage(AppState {
        library: Arc::clone(&library),
    });

    let handle = app.handle().clone();
    watch::start(handle.clone(), folders);

    // Catch up on changes made while Prism was closed; unchanged files are skipped quickly.
    std::thread::spawn(move || {
        let emit = tauri_emitter(handle);
        if let Err(err) = library.rescan_all(&emit) {
            eprintln!("startup rescan failed: {err:#}");
        }
    });
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| setup(app).map_err(|err| err.into()))
        .run(tauri::generate_context!())
        .expect("error while running Prism");
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {
        assert_eq!(2 + 2, 4);
    }
}
