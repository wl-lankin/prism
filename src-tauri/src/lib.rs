pub mod audio;
pub mod commands;
pub mod db;
pub mod library;
pub mod player;
pub mod protocol;
pub mod tools;

use std::sync::Arc;

use anyhow::Context;
use tauri::Manager;

use audio::CpalOutputFactory;
use db::Db;
use library::covers::CoverStore;
use library::scan::lock;
use library::{query, tauri_emitter, watch, Library};
use player::{tauri_sink, Player, Resolver};

/// Shared backend state, managed by Tauri.
pub struct AppState {
    pub library: Arc<Library>,
    pub player: Player,
}

/// Resolves queue entries against the library database.
fn library_resolver(library: &Library) -> Resolver {
    let db = Arc::clone(&library.db);
    Arc::new(move |id: &str| {
        let conn = lock(&db).ok()?;
        query::track(&conn, id).ok().flatten()
    })
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
    let handle = app.handle().clone();
    let player = Player::spawn(
        Arc::new(CpalOutputFactory),
        library_resolver(&library),
        tauri_sink(handle.clone()),
    );
    app.manage(AppState {
        library: Arc::clone(&library),
        player,
    });

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
        .register_asynchronous_uri_scheme_protocol("prism", protocol::register)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| setup(app).map_err(|err| err.into()))
        .invoke_handler(tauri::generate_handler![
            commands::player_play,
            commands::player_toggle,
            commands::player_next,
            commands::player_prev,
            commands::player_seek,
            commands::player_set_volume,
            commands::player_state,
            commands::library_add_folder,
            commands::library_remove_folder,
            commands::library_folders,
            commands::library_rescan,
            commands::library_albums,
            commands::library_album_tracks,
            commands::library_artists,
            commands::library_artist_albums,
            commands::library_track_count,
            commands::library_tracks_range,
            commands::library_search,
        ])
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
