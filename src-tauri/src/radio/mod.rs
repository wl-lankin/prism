//! Internet radio: the station store, the directory client, the location, the "near you"
//! ranking, station logos, and the service that ties them together for the commands.
//!
//! Every method of [`RadioService`] blocks (directory and location requests take seconds), so
//! commands call it off the async runtime. Network work never runs under the DB lock, and the
//! DB lock is never taken while one of the service's own locks is held.

pub mod covers;
pub mod directory;
pub mod location;
pub mod model;
pub mod near;
pub mod store;
pub mod stream;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testserver;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::library::favorites::{self, FavKind};
use crate::library::model::Source;
use crate::library::scan::lock;
use crate::player::Resolver;
use crate::settings::SettingsStore;
use directory::{
    clean_text, Country, Directory, DirectoryError, MirrorSource, Region, StationQuery, TagCount,
    MIRROR_TTL,
};
use location::{place_from_json, place_to_json, resolve_place_with, Locator, Place, PlaceOrigin};
use model::{clean_name, clean_tags, http_url, station_uuid, Station, RADIO_PREFIX};
use near::{NEAR_MAX, NEAR_RADIUS_KM};

/// `{ stationId, title | null }` whenever the stream title of the current station changes.
pub const EVENT_NOW_PLAYING: &str = "radio://now-playing";
/// Played stations that are not favorites are kept this many.
pub const RECENT_KEEP: usize = 30;
/// A near list is asked again after this long.
pub const NEAR_TTL_MS: i64 = 6 * 60 * 60 * 1000;
/// A station's play is reported to the directory at most this often.
pub const CLICK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;
const TOP_TAGS: u32 = 40;
const STATE_PLACE: &str = "place";
const STATE_MIRRORS: &str = "mirrors";
const STATE_NEAR: &str = "near:";
/// Near lists of this many places stay cached (home, a manual choice, one more).
const NEAR_KEEP: usize = 3;

pub use crate::audio::live::TitleSink;
/// Receives every title change of the current station, with its id (for the media controls).
pub type OnTitle = Arc<dyn Fn(&str, Option<String>) + Send + Sync>;
/// A stored station by id, for the radio opener.
pub type StationLookup = Arc<dyn Fn(&str) -> Option<Station> + Send + Sync>;
/// The system locale (`de-DE`), asked only when no other place is known.
pub type LocaleSource = Arc<dyn Fn() -> String + Send + Sync>;

/// The directory calls the service makes; [`Directory`] in the app, the same type against a
/// fake server in tests.
pub trait DirectoryApi: Send + Sync {
    fn search(&self, query: &StationQuery) -> Result<Vec<Station>, DirectoryError>;
    fn by_uuids(&self, uuids: &[&str]) -> Result<Vec<Station>, DirectoryError>;
    fn countries(&self) -> Result<Vec<Country>, DirectoryError>;
    fn states(&self, country_name: &str) -> Result<Vec<Region>, DirectoryError>;
    fn top_tags(&self, limit: u32) -> Result<Vec<TagCount>, DirectoryError>;
    fn click(&self, uuid: &str) -> Result<(), DirectoryError>;
    /// Asks for the mirror list and switches to it; the names go into the cache.
    fn refresh_mirrors(&self) -> Result<Vec<String>, DirectoryError>;
}

impl DirectoryApi for Directory {
    fn search(&self, query: &StationQuery) -> Result<Vec<Station>, DirectoryError> {
        Directory::search(self, query)
    }
    fn by_uuids(&self, uuids: &[&str]) -> Result<Vec<Station>, DirectoryError> {
        Directory::by_uuids(self, uuids)
    }
    fn countries(&self) -> Result<Vec<Country>, DirectoryError> {
        Directory::countries(self)
    }
    fn states(&self, country_name: &str) -> Result<Vec<Region>, DirectoryError> {
        Directory::states(self, country_name)
    }
    fn top_tags(&self, limit: u32) -> Result<Vec<TagCount>, DirectoryError> {
        Directory::top_tags(self, limit)
    }
    fn click(&self, uuid: &str) -> Result<(), DirectoryError> {
        Directory::click(self, uuid)
    }
    fn refresh_mirrors(&self) -> Result<Vec<String>, DirectoryError> {
        Directory::refresh_mirrors(self)
    }
}

/// Stands in when no directory client could be built: everything is unreachable, saved
/// stations still play.
pub struct NoDirectory;

impl NoDirectory {
    fn down<T>() -> Result<T, DirectoryError> {
        Err(DirectoryError::Unreachable("the directory".to_owned()))
    }
}

impl DirectoryApi for NoDirectory {
    fn search(&self, _: &StationQuery) -> Result<Vec<Station>, DirectoryError> {
        NoDirectory::down()
    }
    fn by_uuids(&self, _: &[&str]) -> Result<Vec<Station>, DirectoryError> {
        NoDirectory::down()
    }
    fn countries(&self) -> Result<Vec<Country>, DirectoryError> {
        NoDirectory::down()
    }
    fn states(&self, _: &str) -> Result<Vec<Region>, DirectoryError> {
        NoDirectory::down()
    }
    fn top_tags(&self, _: u32) -> Result<Vec<TagCount>, DirectoryError> {
        NoDirectory::down()
    }
    fn click(&self, _: &str) -> Result<(), DirectoryError> {
        NoDirectory::down()
    }
    fn refresh_mirrors(&self) -> Result<Vec<String>, DirectoryError> {
        NoDirectory::down()
    }
}

/// The mirror list as `radio_state` key `mirrors` holds it.
#[derive(Debug, Serialize, Deserialize)]
struct MirrorCache {
    at: i64,
    hosts: Vec<String>,
}

/// The cached mirror host names, of any age (an old list beats the single fallback).
pub fn cached_mirrors(db: &Db) -> Vec<String> {
    lock(db)
        .ok()
        .and_then(|conn| store::state_get(&conn, STATE_MIRRORS).ok().flatten())
        .and_then(|json| serde_json::from_str::<MirrorCache>(&json).ok())
        .map(|cache| cache.hosts)
        .unwrap_or_default()
}

/// The directory client: the cached mirrors, or `fixed` (a dev override, whose mirrors are
/// never cached). The second value says whether the mirror list is discovered and cached.
pub fn open_directory(db: &Db, fixed: Option<String>) -> (Arc<dyn DirectoryApi>, bool) {
    let (source, discover) = match fixed {
        Some(url) => (MirrorSource::Fixed(vec![url]), false),
        None => (
            MirrorSource::Discover {
                cached: cached_mirrors(db),
            },
            true,
        ),
    };
    match Directory::new(source) {
        Ok(directory) => (Arc::new(directory), discover),
        Err(err) => {
            eprintln!("the radio directory is off: {err:#}");
            (Arc::new(NoDirectory), false)
        }
    }
}

/// A stored station by id. Reads only the DB, so saved stations play without the directory.
pub fn lookup_station(db: Arc<Db>) -> StationLookup {
    Arc::new(move |id: &str| {
        let conn = lock(&db).ok()?;
        store::station(&conn, id).ok().flatten()
    })
}

/// Resolves `radio:` ids from the station store and everything else through `fallback`.
pub fn resolver(db: Arc<Db>, fallback: Resolver) -> Resolver {
    Arc::new(move |id: &str| {
        if id.starts_with(RADIO_PREFIX) {
            let conn = lock(&db).ok()?;
            store::track(&conn, id).ok().flatten()
        } else {
            fallback(id)
        }
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearResult {
    pub stations: Vec<Station>,
    pub place: Place,
    /// The list comes from the cache.
    pub cached: bool,
    /// The cached list is older than 6 hours.
    pub stale: bool,
    /// Why the directory could not be asked (`unreachable`, `server`, `badResponse`).
    pub error: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub favorites: Vec<Station>,
    pub recent: Vec<Station>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub station_id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RadioEvent {
    NowPlaying(NowPlaying),
    /// A station favorite changed (`favorites://changed`).
    FavoritesChanged,
}

pub type RadioEmit = Arc<dyn Fn(RadioEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct RadioOptions {
    /// The first refresh of the stored stations.
    pub first_refresh: Duration,
    pub refresh_every: Duration,
    /// Ask for the mirror list when the cached one is older than [`MIRROR_TTL`]. Off for a
    /// fixed directory.
    pub discover_mirrors: bool,
}

impl Default for RadioOptions {
    fn default() -> Self {
        RadioOptions {
            first_refresh: Duration::from_secs(30),
            refresh_every: Duration::from_secs(24 * 60 * 60),
            discover_mirrors: false,
        }
    }
}

/// The near list as `radio_state` key `near:<place>` holds it.
#[derive(Debug, Serialize, Deserialize)]
struct NearCache {
    at: i64,
    stations: Vec<Station>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fresh(at: i64, now: i64, ttl_ms: i64) -> bool {
    (0..ttl_ms).contains(&(now - at))
}

/// The near cache key of a place: country, region and rounded coordinates, so a move or a
/// manual choice gets its own list. Holds no more than the stored place does.
fn place_key(place: &Place) -> String {
    let region = place.region.as_deref().unwrap_or("").to_lowercase();
    let coords = match (place.origin, place.lat, place.lon) {
        (PlaceOrigin::Manual, _, _) => "-".to_owned(),
        (_, Some(lat), Some(lon)) => format!("{lat:.1},{lon:.1}"),
        _ => "-".to_owned(),
    };
    format!("{}|{region}|{coords}", place.country_code)
}

/// A station that came in through a command, cleaned like a directory answer. `None` when it
/// is not a playable station.
fn sanitize(station: Station) -> Option<Station> {
    station_uuid(&station.id)?;
    let url = http_url(station.url.trim())?.to_owned();
    let keep_url = |value: Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .and_then(http_url)
            .map(str::to_owned)
    };
    let country_code = station
        .country_code
        .filter(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()))
        .map(|code| code.to_ascii_uppercase());
    let favicon = keep_url(station.favicon);
    let homepage = keep_url(station.homepage);
    let cover_id = model::station_cover_id(favicon.as_deref(), homepage.as_deref());
    Some(Station {
        id: station.id,
        name: clean_name(&station.name)?,
        url,
        url_resolved: keep_url(station.url_resolved),
        homepage,
        favicon,
        cover_id,
        country_code,
        region: station.region.as_deref().and_then(|r| clean_text(r, 60)),
        language: station.language.as_deref().and_then(|l| clean_text(l, 60)),
        tags: clean_tags(&station.tags.join(",")),
        codec: station.codec.as_deref().and_then(|c| clean_text(c, 20)),
        bitrate_kbps: station
            .bitrate_kbps
            .filter(|rate| (1..10_000).contains(rate)),
        hls: station.hls,
        last_check_ok: station.last_check_ok,
        distance_km: None,
    })
}

/// Radio for the commands: the place, near you, search, saved stations, clicks, the stream
/// title, and a daily refresh of the stored stations.
pub struct RadioService {
    me: Weak<RadioService>,
    db: Arc<Db>,
    settings: Arc<SettingsStore>,
    directory: Arc<dyn DirectoryApi>,
    locator: Arc<dyn Locator>,
    locale_source: LocaleSource,
    locale: OnceLock<String>,
    emit: RadioEmit,
    on_title: OnTitle,
    options: RadioOptions,
    countries: Mutex<Option<Vec<Country>>>,
    regions: Mutex<HashMap<String, Vec<Region>>>,
    tags: Mutex<Option<Vec<TagCount>>>,
    current: Mutex<Option<NowPlaying>>,
    mirrors_checked: AtomicBool,
    /// True once [`RadioService::shutdown`] ran; wakes the refresh thread.
    stop: Arc<(Mutex<bool>, Condvar)>,
}

impl Drop for RadioService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl RadioService {
    /// `on_title` gets every stream title change too (for the media controls).
    pub fn spawn(
        db: Arc<Db>,
        settings: Arc<SettingsStore>,
        directory: Arc<dyn DirectoryApi>,
        locator: Arc<dyn Locator>,
        locale: LocaleSource,
        emit: RadioEmit,
        on_title: OnTitle,
    ) -> Arc<RadioService> {
        RadioService::spawn_with_options(
            db,
            settings,
            directory,
            locator,
            locale,
            emit,
            on_title,
            RadioOptions::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_options(
        db: Arc<Db>,
        settings: Arc<SettingsStore>,
        directory: Arc<dyn DirectoryApi>,
        locator: Arc<dyn Locator>,
        locale: LocaleSource,
        emit: RadioEmit,
        on_title: OnTitle,
        options: RadioOptions,
    ) -> Arc<RadioService> {
        let service = Arc::new_cyclic(|me| RadioService {
            me: me.clone(),
            db,
            settings,
            directory,
            locator,
            locale_source: locale,
            locale: OnceLock::new(),
            emit,
            on_title,
            options,
            countries: Mutex::new(None),
            regions: Mutex::new(HashMap::new()),
            tags: Mutex::new(None),
            current: Mutex::new(None),
            mirrors_checked: AtomicBool::new(false),
            stop: Arc::new((Mutex::new(false), Condvar::new())),
        });
        service.start_refresh();
        service
    }

    /// Ends the refresh thread.
    pub fn shutdown(&self) {
        let (stopped, wake) = &*self.stop;
        *guard(stopped) = true;
        wake.notify_all();
    }

    fn locale(&self) -> &str {
        // Only the locale fallback of `resolve` asks, so macOS spawns `defaults` at most once
        // and only when nothing else names a country.
        self.locale.get_or_init(|| (self.locale_source)())
    }

    fn stored_place(&self) -> Option<Place> {
        let conn = lock(&self.db).ok()?;
        store::state_get(&conn, STATE_PLACE)
            .ok()
            .flatten()
            .and_then(|json| place_from_json(&json))
    }

    fn state_set(&self, key: &str, value: &str) {
        let written = lock(&self.db).and_then(|conn| store::state_set(&conn, key, value));
        if let Err(err) = written {
            eprintln!("writing the radio state failed: {err:#}");
        }
    }

    /// Where the user is: the manual choice, the stored lookup while it is younger than 7
    /// days, a new lookup, or the locale's country. A lookup's place is stored.
    pub fn place(&self) -> Place {
        self.resolve(false)
    }

    /// Like [`RadioService::place`], but looks the place up again (automatic mode only). When
    /// that fails, the stored place stays.
    pub fn redetect(&self) -> Place {
        self.resolve(true)
    }

    fn resolve(&self, again: bool) -> Place {
        let setting = self.settings.get().radio_location;
        let stored = self.stored_place();
        let cached = match (&stored, again) {
            (Some(place), true) => Some(Place {
                detected_at: None,
                ..place.clone()
            }),
            _ => stored.clone(),
        };
        let (place, store_it) = resolve_place_with(
            &setting,
            cached,
            self.locator.as_ref(),
            &|| self.locale().to_owned(),
            now_ms(),
        );
        if store_it {
            self.state_set(STATE_PLACE, &place_to_json(&place));
            return place;
        }
        match stored {
            Some(stored) if again && place.origin == PlaceOrigin::Detected => stored,
            _ => place,
        }
    }

    /// The stations near the user. A list younger than 6 hours comes from the cache unless
    /// `refresh`; when the directory cannot be asked, the cached list (of any age) comes back
    /// with the error.
    pub fn near(&self, refresh: bool) -> NearResult {
        let place = self.place();
        let key = format!("{STATE_NEAR}{}", place_key(&place));
        let cached = lock(&self.db)
            .ok()
            .and_then(|conn| store::state_get(&conn, &key).ok().flatten())
            .and_then(|json| serde_json::from_str::<NearCache>(&json).ok());
        let now = now_ms();
        if let Some(cache) = &cached {
            if !refresh && fresh(cache.at, now, NEAR_TTL_MS) {
                return NearResult {
                    stations: cache.stations.clone(),
                    place,
                    cached: true,
                    stale: false,
                    error: None,
                };
            }
        }
        match self.fetch_near(&place) {
            Ok(stations) => {
                let json = serde_json::to_string(&NearCache {
                    at: now,
                    stations: stations.clone(),
                })
                .unwrap_or_default();
                let written = lock(&self.db).and_then(|conn| {
                    store::state_set(&conn, &key, &json)?;
                    store::prune_near_state(&conn, STATE_NEAR, NEAR_KEEP)?;
                    store::remember_favicons(&conn, &stations)
                });
                if let Err(err) = written {
                    eprintln!("caching the near list failed: {err:#}");
                }
                NearResult {
                    stations,
                    place,
                    cached: false,
                    stale: false,
                    error: None,
                }
            }
            Err(err) => match cached {
                Some(cache) => NearResult {
                    stale: !fresh(cache.at, now, NEAR_TTL_MS),
                    stations: cache.stations,
                    place,
                    cached: true,
                    error: Some(err.code()),
                },
                None => NearResult {
                    stations: Vec::new(),
                    place,
                    cached: false,
                    stale: false,
                    error: Some(err.code()),
                },
            },
        }
    }

    fn fetch_near(&self, place: &Place) -> Result<Vec<Station>, DirectoryError> {
        self.directory_used();
        let country = || StationQuery {
            country_code: Some(place.country_code.clone()),
            limit: NEAR_MAX as u32,
            ..StationQuery::default()
        };
        let geo = match (place.origin, place.lat, place.lon) {
            (PlaceOrigin::Manual, _, _) => Vec::new(),
            (_, Some(lat), Some(lon)) => self.directory.search(&StationQuery {
                geo: Some((lat, lon, f64::from(NEAR_RADIUS_KM) * 1000.0)),
                limit: NEAR_MAX as u32,
                ..StationQuery::default()
            })?,
            _ => Vec::new(),
        };
        let region = match place.region.as_deref() {
            Some(wanted) => match self.region_name(&place.country_code, wanted)? {
                Some(name) => self.directory.search(&StationQuery {
                    region: Some(name),
                    ..country()
                })?,
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let country = self.directory.search(&country())?;
        Ok(near::rank(place, geo, region, country, NEAR_MAX))
    }

    /// The directory's name of the region `wanted` (any case) in a country.
    fn region_name(&self, code: &str, wanted: &str) -> Result<Option<String>, DirectoryError> {
        let wanted = wanted.trim().to_lowercase();
        Ok(self
            .regions(code)?
            .into_iter()
            .find(|region| region.name.to_lowercase() == wanted)
            .map(|region| region.name))
    }

    /// A station search; the logos of the results are remembered so their tiles load.
    pub fn search(&self, query: StationQuery) -> Result<Vec<Station>, DirectoryError> {
        self.directory_used();
        let stations = self.directory.search(&query)?;
        let written = lock(&self.db).and_then(|conn| store::remember_favicons(&conn, &stations));
        if let Err(err) = written {
            eprintln!("remembering station logos failed: {err:#}");
        }
        Ok(stations)
    }

    /// Countries with stations, asked once per session.
    pub fn countries(&self) -> Result<Vec<Country>, DirectoryError> {
        if let Some(countries) = guard(&self.countries).clone() {
            return Ok(countries);
        }
        self.directory_used();
        let countries = self.directory.countries()?;
        *guard(&self.countries) = Some(countries.clone());
        Ok(countries)
    }

    /// The regions of a country, asked once per country and session. A country the directory
    /// does not list has none.
    pub fn regions(&self, country_code: &str) -> Result<Vec<Region>, DirectoryError> {
        let code = country_code.trim().to_ascii_uppercase();
        if let Some(regions) = guard(&self.regions).get(&code).cloned() {
            return Ok(regions);
        }
        let Some(name) = self
            .countries()?
            .into_iter()
            .find(|country| country.code == code)
            .map(|country| country.name)
        else {
            return Ok(Vec::new());
        };
        let regions = self.directory.states(&name)?;
        guard(&self.regions).insert(code, regions.clone());
        Ok(regions)
    }

    /// The most used genres, asked once per session.
    pub fn top_tags(&self) -> Result<Vec<TagCount>, DirectoryError> {
        if let Some(tags) = guard(&self.tags).clone() {
            return Ok(tags);
        }
        self.directory_used();
        let tags = self.directory.top_tags(TOP_TAGS)?;
        *guard(&self.tags) = Some(tags.clone());
        Ok(tags)
    }

    /// Stores a station the user is about to play, marks it played, prunes the recent list
    /// and reports the play to the directory (in the background, at most once a day per
    /// station). Returns the id to play.
    pub fn play_prepared(&self, station: Station) -> anyhow::Result<String> {
        let station = sanitize(station).context("this is not a playable station")?;
        let id = station.id.clone();
        let now = now_ms();
        let click = {
            let conn = lock(&self.db)?;
            store::upsert_station(&conn, &station, now)?;
            store::mark_played(&conn, &id, now)?;
            store::prune(&conn, RECENT_KEEP)?;
            store::claim_click(&conn, &id, now, CLICK_INTERVAL_MS)?
        };
        self.set_current(&id);
        if click {
            let directory = Arc::clone(&self.directory);
            let uuid = station_uuid(&id).unwrap_or_default().to_owned();
            let spawned = std::thread::Builder::new()
                .name("prism-radio-click".into())
                .spawn(move || {
                    if let Err(err) = directory.click(&uuid) {
                        eprintln!("reporting a station play failed: {err}");
                    }
                });
            if let Err(err) = spawned {
                eprintln!("reporting a station play failed: {err}");
            }
        }
        Ok(id)
    }

    /// Favorite stations and the recently played others.
    pub fn saved(&self) -> anyhow::Result<Saved> {
        let conn = lock(&self.db)?;
        Ok(Saved {
            favorites: favorites::stations(&conn)?,
            recent: store::recent(&conn, RECENT_KEEP)?,
        })
    }

    /// A heart on a station: stores it (on) or lets it go once it is not recent either
    /// (off). Returns whether anything changed; a change emits `favorites://changed`.
    pub fn set_favorite(&self, station: Station, on: bool) -> anyhow::Result<bool> {
        let station = if on {
            sanitize(station).context("this is not a station")?
        } else {
            station_uuid(&station.id).context("this is not a station")?;
            station
        };
        let changed = {
            let conn = lock(&self.db)?;
            if on {
                store::upsert_station(&conn, &station, now_ms())?;
            }
            let changed = favorites::set_with_source(
                &conn,
                FavKind::Station,
                &station.id,
                on,
                Source::Radio,
                now_ms(),
            )?;
            if !on {
                store::prune(&conn, RECENT_KEEP)?;
            }
            changed
        };
        if changed {
            (self.emit)(RadioEvent::FavoritesChanged);
        }
        Ok(changed)
    }

    /// [`lookup_station`] that also notes the station as the current one: the opener looks a
    /// station up right before it plays, so the titles that follow belong to it.
    pub fn lookup(&self) -> StationLookup {
        let lookup = lookup_station(Arc::clone(&self.db));
        let me = self.me.clone();
        Arc::new(move |id: &str| {
            let station = lookup(id)?;
            if let Some(service) = me.upgrade() {
                service.set_current(&station.id);
            }
            Some(station)
        })
    }

    fn set_current(&self, id: &str) {
        let mut current = guard(&self.current);
        if current.as_ref().map(|now| now.station_id.as_str()) != Some(id) {
            *current = Some(NowPlaying {
                station_id: id.to_owned(),
                title: None,
            });
        }
    }

    /// Where the opener of `station_id` sends its stream titles. A title counts only while
    /// that station is the current one (a stream still closing never titles the next
    /// station); it is kept, emitted as `radio://now-playing` and handed to `on_title`. An
    /// unchanged title is not sent again.
    pub fn title_sink_for(&self, station_id: &str) -> TitleSink {
        let me = self.me.clone();
        let station_id = station_id.to_owned();
        Arc::new(move |title: Option<String>| {
            if let Some(service) = me.upgrade() {
                service.stream_title(&station_id, title);
            }
        })
    }

    /// [`RadioService::title_sink_for`] as a factory, for the radio opener.
    pub fn title_sinks(&self) -> Arc<dyn Fn(&str) -> TitleSink + Send + Sync> {
        let me = self.me.clone();
        Arc::new(move |station_id: &str| match me.upgrade() {
            Some(service) => service.title_sink_for(station_id),
            None => Arc::new(|_| {}),
        })
    }

    fn stream_title(&self, station_id: &str, title: Option<String>) {
        let title = title.filter(|title| !title.trim().is_empty());
        let event = {
            let mut current = guard(&self.current);
            let Some(now) = current.as_mut().filter(|now| now.station_id == station_id) else {
                return;
            };
            if now.title == title {
                return;
            }
            now.title = title.clone();
            now.clone()
        };
        (self.emit)(RadioEvent::NowPlaying(event));
        (self.on_title)(station_id, title);
    }

    /// The current station and its title, for windows that load later.
    pub fn now_playing(&self) -> Option<NowPlaying> {
        guard(&self.current).clone()
    }

    /// Asks the directory for every stored station and writes what changed. A station the
    /// directory does not know any more is kept as it is (it may still stream). Returns how
    /// many changed.
    pub(crate) fn refresh_stations(&self) -> Result<usize, DirectoryError> {
        let stored = match lock(&self.db).and_then(|conn| store::all(&conn)) {
            Ok(stored) => stored,
            Err(err) => {
                eprintln!("reading the saved stations failed: {err:#}");
                return Ok(0);
            }
        };
        // Without a stored station nobody uses radio: no request at all, mirrors included.
        if stored.is_empty() {
            return Ok(0);
        }
        // On the refresh thread already, so the mirror check runs here, once a day.
        self.mirrors_checked.store(true, Ordering::SeqCst);
        self.check_mirrors();
        let uuids: Vec<&str> = stored
            .iter()
            .filter_map(|station| station_uuid(&station.id))
            .collect();
        let known: HashMap<&str, &Station> = stored
            .iter()
            .map(|station| (station.id.as_str(), station))
            .collect();
        let changed: Vec<Station> = self
            .directory
            .by_uuids(&uuids)?
            .into_iter()
            .filter(|fresh| {
                known
                    .get(fresh.id.as_str())
                    .is_some_and(|old| **old != *fresh)
            })
            .collect();
        let written = lock(&self.db).and_then(|conn| {
            let now = now_ms();
            let mut updated = 0;
            for station in &changed {
                // A station pruned while the directory answered stays gone.
                if store::station(&conn, &station.id)?.is_some() {
                    store::upsert_station(&conn, station, now)?;
                    updated += 1;
                }
            }
            Ok(updated)
        });
        match written {
            Ok(updated) => Ok(updated),
            Err(err) => {
                eprintln!("writing refreshed stations failed: {err:#}");
                Ok(0)
            }
        }
    }

    /// Refreshes the mirror list in the background the first time the directory is used in
    /// a session, when the cached one is old.
    fn directory_used(&self) {
        if !self.options.discover_mirrors || self.mirrors_checked.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.me.clone();
        let spawned = std::thread::Builder::new()
            .name("prism-radio-mirrors".into())
            .spawn(move || {
                if let Some(service) = me.upgrade() {
                    service.check_mirrors();
                }
            });
        if let Err(err) = spawned {
            eprintln!("refreshing the radio mirrors failed: {err}");
        }
    }

    fn check_mirrors(&self) {
        if !self.options.discover_mirrors {
            return;
        }
        let now = now_ms();
        let at = lock(&self.db)
            .ok()
            .and_then(|conn| store::state_get(&conn, STATE_MIRRORS).ok().flatten())
            .and_then(|json| serde_json::from_str::<MirrorCache>(&json).ok())
            .map(|cache| cache.at);
        if at.is_some_and(|at| fresh(at, now, MIRROR_TTL.as_millis() as i64)) {
            return;
        }
        match self.directory.refresh_mirrors() {
            Ok(hosts) => {
                let json =
                    serde_json::to_string(&MirrorCache { at: now, hosts }).unwrap_or_default();
                self.state_set(STATE_MIRRORS, &json);
            }
            Err(err) => eprintln!("refreshing the radio mirrors failed: {err}"),
        }
    }

    fn start_refresh(&self) {
        let me = self.me.clone();
        let stop = Arc::clone(&self.stop);
        let (first, every) = (self.options.first_refresh, self.options.refresh_every);
        let spawned = std::thread::Builder::new()
            .name("prism-radio-refresh".into())
            .spawn(move || {
                let mut wait = first;
                loop {
                    {
                        let (stopped, wake) = &*stop;
                        let stopped = guard(stopped);
                        let (stopped, _) = wake
                            .wait_timeout_while(stopped, wait, |stopped| !*stopped)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if *stopped {
                            return;
                        }
                    }
                    let Some(service) = me.upgrade() else {
                        return;
                    };
                    if let Err(err) = service.refresh_stations() {
                        eprintln!("refreshing the saved stations failed: {err}");
                    }
                    drop(service);
                    wait = every;
                }
            });
        if let Err(err) = spawned {
            eprintln!("starting the radio refresh failed: {err}");
        }
    }
}
