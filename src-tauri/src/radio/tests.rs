use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use serde_json::json;
use tempfile::TempDir;

use super::directory::Order;
use super::location::LocateError;
use super::testserver::{FakeDirectory, FakeStation};
use super::*;
use crate::library::model::Track;

const WAIT: Duration = Duration::from_secs(10);

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Testhausen, or nothing when `fail` is set; counts the lookups.
struct FakeLocator {
    calls: AtomicUsize,
    fail: AtomicBool,
}

impl Locator for FakeLocator {
    fn locate(&self) -> Result<Place, LocateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(LocateError::Unreachable);
        }
        Ok(Place {
            country_code: "DE".into(),
            region: Some("Testland".into()),
            city: Some("Testhausen".into()),
            lat: Some(50.0),
            lon: Some(10.0),
            origin: PlaceOrigin::Detected,
            detected_at: None,
        })
    }
}

struct Env {
    _dir: TempDir,
    db: Arc<Db>,
    settings: Arc<SettingsStore>,
    fake: FakeDirectory,
    locator: Arc<FakeLocator>,
    events: Arc<Mutex<Vec<RadioEvent>>>,
    titles: Arc<Mutex<Vec<Option<String>>>>,
    service: Arc<RadioService>,
}

impl Env {
    fn start(stations: Vec<FakeStation>) -> Env {
        Env::start_with(
            stations,
            RadioOptions {
                first_refresh: Duration::from_secs(3600),
                ..RadioOptions::default()
            },
        )
    }

    fn start_with(stations: Vec<FakeStation>, options: RadioOptions) -> Env {
        let fake = FakeDirectory::start(stations);
        let directory = Directory::new(MirrorSource::Fixed(vec![fake.url()])).unwrap();
        Env::with_directory(fake, Arc::new(directory), options)
    }

    fn with_directory(
        fake: FakeDirectory,
        directory: Arc<dyn DirectoryApi>,
        options: RadioOptions,
    ) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let settings = Arc::new(SettingsStore::load(&dir.path().join("settings.json")));
        let locator = Arc::new(FakeLocator {
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        });
        let events: Arc<Mutex<Vec<RadioEvent>>> = Arc::default();
        let titles: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
        let service = {
            let events = Arc::clone(&events);
            let titles = Arc::clone(&titles);
            RadioService::spawn_with_options(
                Arc::clone(&db),
                Arc::clone(&settings),
                directory,
                Arc::clone(&locator) as Arc<dyn Locator>,
                Arc::new(|| "de-DE".to_owned()),
                Arc::new(move |event| events.lock().unwrap().push(event)),
                Arc::new(move |title| titles.lock().unwrap().push(title)),
                options,
            )
        };
        Env {
            _dir: dir,
            db,
            settings,
            fake,
            locator,
            events,
            titles,
            service,
        }
    }

    fn manual(&self, country: &str, region: Option<&str>) {
        self.settings
            .set(json!({ "radioLocation": {
                "mode": "manual", "countryCode": country, "region": region } }))
            .unwrap();
    }

    fn requests(&self, path: &str) -> Vec<super::testserver::Recorded> {
        self.fake
            .requests()
            .into_iter()
            .filter(|r| r.path == path)
            .collect()
    }

    fn stored(&self, id: &str) -> Option<Station> {
        let conn = self.db.0.lock().unwrap();
        store::station(&conn, id).unwrap()
    }

    fn state(&self, key: &str) -> Option<String> {
        let conn = self.db.0.lock().unwrap();
        store::state_get(&conn, key).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.service.shutdown();
    }
}

fn numbers(stations: &[Station]) -> Vec<u32> {
    stations
        .iter()
        .map(|s| u32::from_str_radix(&s.id[s.id.len() - 12..], 16).unwrap())
        .collect()
}

/// A station as a search would hand it to the UI.
fn found(env: &Env, n: u32) -> Station {
    let query = StationQuery {
        name: Some(format!("Demo Radio {n}")),
        order: Order::Name,
        ..StationQuery::default()
    };
    env.service
        .search(query)
        .unwrap()
        .into_iter()
        .find(|s| s.name == format!("Demo Radio {n}"))
        .unwrap()
}

/// Two stations within 75 km of Testhausen, three more in Testland without coordinates, five
/// elsewhere in Germany (one of them far away with coordinates), one in Austria.
fn around_testhausen() -> Vec<FakeStation> {
    vec![
        FakeStation::new(1).geo(50.1, 10.1).clicks(50),
        FakeStation::new(2).geo(50.3, 10.2).clicks(90),
        FakeStation::new(3).clicks(30),
        FakeStation::new(4).clicks(10),
        FakeStation::new(5).clicks(20),
        FakeStation::new(6).region("Elsewhere").clicks(9),
        FakeStation::new(7).region("Elsewhere").clicks(7),
        FakeStation::new(8)
            .region("Elsewhere")
            .geo(52.5, 13.4)
            .clicks(8),
        FakeStation::new(9).region("Elsewhere").clicks(6),
        FakeStation::new(10).region("Elsewhere").clicks(5),
        FakeStation::new(11)
            .country("AT")
            .region("Elsewhere")
            .clicks(100),
    ]
}

#[test]
fn near_list_from_the_directory_in_band_order() {
    let env = Env::start(around_testhausen());
    let near = env.service.near(false);
    assert_eq!(near.error, None);
    assert!(!near.cached);
    assert_eq!(near.place.city.as_deref(), Some("Testhausen"));
    assert_eq!(numbers(&near.stations), vec![2, 1, 3, 5, 4, 6, 8, 7, 9, 10]);
    assert!(near.stations[..2].iter().all(|s| s.distance_km.is_some()));
    assert!(near.stations[2..].iter().all(|s| s.distance_km.is_none()));
    let searches = env.requests("/json/stations/search");
    assert_eq!(searches.len(), 3);
    assert_eq!(searches[0].param("geo_distance"), Some("75000"));
    assert_eq!(searches[1].param("state"), Some("Testland"));
    assert_eq!(searches[2].param("countrycode"), Some("DE"));
    // The tiles of the list can load.
    let conn = env.db.0.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM radio_stations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "near results are not stored as stations");
}

#[test]
fn a_fresh_near_list_makes_no_request() {
    let env = Env::start(around_testhausen());
    let first = env.service.near(false);
    let before = env.fake.requests().len();
    let again = env.service.near(false);
    assert_eq!(env.fake.requests().len(), before);
    assert!(again.cached && !again.stale && again.error.is_none());
    assert_eq!(again.stations, first.stations);
    // Refresh asks again.
    assert!(!env.service.near(true).cached);
    assert!(env.fake.requests().len() > before);
}

#[test]
fn manual_place_skips_the_geo_band() {
    let env = Env::start(around_testhausen());
    env.manual("DE", Some("testland"));
    let near = env.service.near(false);
    assert_eq!(near.place.origin, PlaceOrigin::Manual);
    assert_eq!(numbers(&near.stations), vec![2, 1, 3, 5, 4, 6, 8, 7, 9, 10]);
    assert!(near.stations.iter().all(|s| s.distance_km.is_none()));
    assert!(env
        .fake
        .requests()
        .iter()
        .all(|r| r.param("geo_lat").is_none()));
    assert_eq!(env.locator.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn near_list_falls_back_to_cache() {
    let mut env = Env::start(around_testhausen());
    let first = env.service.near(false);
    env.fake.stop();
    let down = env.service.near(true);
    assert_eq!(down.stations, first.stations);
    assert!(down.cached && !down.stale);
    assert_eq!(down.error, Some("unreachable"));

    // Seven hours later the cached list is stale, and still shown.
    let key = format!("{STATE_NEAR}{}", place_key(&first.place));
    let old = json!({ "at": now_ms() - 7 * 60 * 60 * 1000, "stations": first.stations });
    env.service.state_set(&key, &old.to_string());
    let stale = env.service.near(false);
    assert_eq!(stale.stations, first.stations);
    assert!(stale.cached && stale.stale);
    assert_eq!(stale.error, Some("unreachable"));

    // Nothing cached for another place: an empty list with the error.
    env.manual("AT", None);
    let empty = env.service.near(false);
    assert!(empty.stations.is_empty() && !empty.cached);
    assert_eq!(empty.error, Some("unreachable"));
}

#[test]
fn near_cache_is_per_place() {
    let mut env = Env::start(around_testhausen());
    env.manual("DE", None);
    let germany = env.service.near(false);
    assert_eq!(
        numbers(&germany.stations),
        vec![2, 1, 3, 5, 4, 6, 8, 7, 9, 10]
    );
    env.manual("AT", None);
    let austria = env.service.near(false);
    assert!(!austria.cached);
    assert_eq!(numbers(&austria.stations), vec![11]);
    assert_eq!(
        env.requests("/json/stations/search")
            .last()
            .and_then(|r| r.param("countrycode").map(str::to_owned))
            .as_deref(),
        Some("AT")
    );

    env.fake.stop();
    env.manual("DE", None);
    let back = env.service.near(false);
    assert!(back.cached && back.error.is_none());
    assert_eq!(back.stations, germany.stations);
    env.manual("AT", None);
    assert_eq!(env.service.near(true).stations, austria.stations);
}

#[test]
fn place_is_located_once_and_stored() {
    let env = Env::start(Vec::new());
    let place = env.service.place();
    assert_eq!(place.origin, PlaceOrigin::Detected);
    assert_eq!((place.lat, place.lon), (Some(50.0), Some(10.0)));
    assert_eq!(env.service.place(), place);
    assert_eq!(env.locator.calls.load(Ordering::SeqCst), 1);
    let stored = place_from_json(&env.state(STATE_PLACE).unwrap()).unwrap();
    assert_eq!(stored, place);

    // Detecting again when the service fails keeps the stored place.
    env.locator.fail.store(true, Ordering::SeqCst);
    assert_eq!(env.service.redetect(), place);
    assert_eq!(env.locator.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        place_from_json(&env.state(STATE_PLACE).unwrap()),
        Some(place)
    );
}

#[test]
fn without_a_lookup_the_locale_country_is_used_and_nothing_stored() {
    let env = Env::start(Vec::new());
    env.locator.fail.store(true, Ordering::SeqCst);
    let place = env.service.place();
    assert_eq!(place.origin, PlaceOrigin::Locale);
    assert_eq!(place.country_code, "DE");
    assert_eq!(env.state(STATE_PLACE), None);
    // Manual mode never asks.
    env.manual("AT", Some("Examplestate"));
    let calls = env.locator.calls.load(Ordering::SeqCst);
    assert_eq!(env.service.redetect().country_code, "AT");
    assert_eq!(env.locator.calls.load(Ordering::SeqCst), calls);
}

#[test]
fn play_prepared_stores_marks_and_clicks_once() {
    let env = Env::start(vec![FakeStation::new(1)]);
    let station = found(&env, 1);
    let id = env.service.play_prepared(station.clone()).unwrap();
    assert_eq!(id, station.id);
    assert_eq!(env.stored(&id), Some(station.clone()));
    let clicks = |env: &Env| env.requests(&format!("/json/url/{}", FakeStation::new(1).uuid()));
    wait_for("the click", || clicks(&env).len() == 1);
    env.service.play_prepared(station).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(clicks(&env).len(), 1, "one click within 24 h");
    let saved = env.service.saved().unwrap();
    assert_eq!(numbers(&saved.recent), vec![1]);
    assert!(saved.favorites.is_empty());
}

#[test]
fn hostile_stations_are_cleaned_or_refused() {
    let env = Env::start(vec![FakeStation::new(1)]);
    let good = found(&env, 1);
    let mut bad_id = good.clone();
    bad_id.id = "radio:../../etc".into();
    assert!(env.service.play_prepared(bad_id).is_err());
    let mut bad_url = good.clone();
    bad_url.url = "file:///etc/passwd".into();
    assert!(env.service.play_prepared(bad_url).is_err());
    let mut messy = good.clone();
    messy.name = format!("\u{7}{}", "x".repeat(2000));
    messy.tags = (0..300).map(|n| format!("tag{n}")).collect();
    messy.favicon = Some("javascript:alert(1)".into());
    let id = env.service.play_prepared(messy).unwrap();
    let stored = env.stored(&id).unwrap();
    assert_eq!(stored.name.chars().count(), 80);
    assert_eq!(stored.tags.len(), 8);
    assert_eq!(stored.favicon, None);
}

#[test]
fn saved_stations_play_without_the_directory() {
    let mut env = Env::start(vec![FakeStation::new(1), FakeStation::new(2)]);
    let favorite = found(&env, 1);
    let recent = found(&env, 2);
    assert!(env.service.set_favorite(favorite.clone(), true).unwrap());
    env.service.play_prepared(recent.clone()).unwrap();
    env.fake.stop();

    let local = Track {
        id: "local-1".into(),
        source: Source::Local,
        ..favorite.as_track()
    };
    let library: Resolver = Arc::new(move |id: &str| (id == "local-1").then(|| local.clone()));
    let resolve = resolver(Arc::clone(&env.db), library);
    let lookup = lookup_station(Arc::clone(&env.db));
    for station in [&favorite, &recent] {
        let track = resolve(&station.id).unwrap();
        assert_eq!(track.source, Source::Radio);
        assert_eq!(track.duration_ms, 0);
        assert_eq!(track.title, station.name);
        assert_eq!(lookup(&station.id).unwrap().url, station.url);
    }
    assert!(resolve("local-1").is_some(), "other ids go to the library");
    assert!(resolve("radio:00000000-0000-4000-8000-000000000099").is_none());
    assert!(lookup("radio:00000000-0000-4000-8000-000000000099").is_none());
    let saved = env.service.saved().unwrap();
    assert_eq!(numbers(&saved.favorites), vec![1]);
    assert_eq!(numbers(&saved.recent), vec![2]);
}

#[test]
fn favorite_station_survives_prune() {
    let stations: Vec<FakeStation> = (1..=32).map(FakeStation::new).collect();
    let env = Env::start(stations);
    let all = env
        .service
        .search(StationQuery {
            limit: 100,
            order: Order::Name,
            ..StationQuery::default()
        })
        .unwrap();
    let favorite = all.iter().find(|s| s.name == "Demo Radio 1").unwrap();
    assert!(env.service.set_favorite(favorite.clone(), true).unwrap());
    assert_eq!(
        env.events.lock().unwrap().as_slice(),
        [RadioEvent::FavoritesChanged]
    );
    // The same heart again changes nothing and emits nothing.
    assert!(!env.service.set_favorite(favorite.clone(), true).unwrap());
    assert_eq!(env.events.lock().unwrap().len(), 1);
    env.service.play_prepared(favorite.clone()).unwrap();
    for station in all.iter().filter(|s| s.id != favorite.id) {
        env.service.play_prepared(station.clone()).unwrap();
    }
    assert!(env.stored(&favorite.id).is_some());
    let saved = env.service.saved().unwrap();
    assert_eq!(numbers(&saved.favorites), vec![1]);
    assert_eq!(saved.recent.len(), RECENT_KEEP);

    // Off again: the station goes once it is not among the recent ones.
    assert!(env.service.set_favorite(favorite.clone(), false).unwrap());
    assert!(env.stored(&favorite.id).is_none());
    assert_eq!(env.events.lock().unwrap().len(), 2);
}

#[test]
fn refresh_updates_changed_urls() {
    let old = found(&Env::start(vec![FakeStation::new(1)]), 1);
    // The directory moved the stream and renamed the station.
    let fake = FakeDirectory::start(vec![FakeStation::new(1)
        .name("Demo Radio One")
        .url("http://stream.example.org/moved/1")]);
    let directory = Directory::new(MirrorSource::Fixed(vec![fake.url()])).unwrap();
    let refreshed = Env::with_directory(
        fake,
        Arc::new(directory),
        RadioOptions {
            first_refresh: Duration::from_secs(3600),
            ..RadioOptions::default()
        },
    );
    {
        let conn = refreshed.db.0.lock().unwrap();
        store::upsert_station(&conn, &old, 1).unwrap();
        store::mark_played(&conn, &old.id, 5).unwrap();
    }
    assert_eq!(refreshed.service.refresh_stations(), Ok(1));
    let stored = refreshed.stored(&old.id).unwrap();
    assert_eq!(stored.url, "http://stream.example.org/moved/1");
    assert_eq!(stored.name, "Demo Radio One");
    // Unchanged now: nothing is written.
    assert_eq!(refreshed.service.refresh_stations(), Ok(0));
    let played: i64 = refreshed
        .db
        .0
        .lock()
        .unwrap()
        .query_row("SELECT last_played_at FROM radio_stations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(played, 5, "the play history stays");
}

#[test]
fn refresh_keeps_unknown_stations() {
    let env = Env::start(vec![FakeStation::new(1), FakeStation::new(2)]);
    let gone = Station {
        id: FakeStation::new(77).id(),
        name: "Demo Radio 77".into(),
        url: "http://stream.example.org/stream/77".into(),
        ..found(&env, 1)
    };
    env.service.play_prepared(gone.clone()).unwrap();
    env.service.play_prepared(found(&env, 2)).unwrap();
    assert_eq!(env.service.refresh_stations(), Ok(0));
    assert_eq!(env.stored(&gone.id).as_ref(), Some(&gone));
    let asked = env.requests("/json/stations/byuuid");
    assert_eq!(asked.len(), 1);
    let uuids = asked[0].param("uuids").unwrap();
    assert!(uuids.contains(&FakeStation::new(77).uuid()));
}

#[test]
fn refresh_runs_in_the_background_and_not_without_stations() {
    let env = Env::start_with(
        vec![FakeStation::new(1)],
        RadioOptions {
            first_refresh: Duration::from_millis(300),
            refresh_every: Duration::from_millis(100),
            ..RadioOptions::default()
        },
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        env.requests("/json/stations/byuuid").is_empty(),
        "nothing stored"
    );
    env.service.play_prepared(found(&env, 1)).unwrap();
    wait_for("a background refresh", || {
        !env.requests("/json/stations/byuuid").is_empty()
    });
}

#[test]
fn a_fixed_directory_never_caches_mirrors() {
    let env = Env::start(vec![FakeStation::new(1)]);
    found(&env, 1);
    env.service.check_mirrors();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(env.state(STATE_MIRRORS), None);
    assert!(env.requests("/json/servers").is_empty());
}

#[test]
fn old_mirror_lists_are_refreshed_and_cached() {
    let fake = FakeDirectory::start(vec![FakeStation::new(1)]);
    let mut directory = Directory::new(MirrorSource::Fixed(vec![fake.url()])).unwrap();
    directory.set_discovery_base(&fake.url());
    let env = Env::with_directory(
        fake,
        Arc::new(directory),
        RadioOptions {
            first_refresh: Duration::from_secs(3600),
            discover_mirrors: true,
            ..RadioOptions::default()
        },
    );
    // Nobody uses radio yet: the daily refresh asks nothing, not even for mirrors.
    assert_eq!(env.service.refresh_stations(), Ok(0));
    assert!(env.fake.requests().is_empty());
    // A fresh list is not asked again.
    let fresh_list = json!({ "at": now_ms(), "hosts": ["de1.api.radio-browser.info"] });
    env.service
        .state_set(STATE_MIRRORS, &fresh_list.to_string());
    env.service.check_mirrors();
    assert!(env.requests("/json/servers").is_empty());
    // A week-old one is.
    let old_list = json!({ "at": now_ms() - 8 * 24 * 60 * 60 * 1000, "hosts": [] });
    env.service.state_set(STATE_MIRRORS, &old_list.to_string());
    env.service.check_mirrors();
    assert_eq!(env.requests("/json/servers").len(), 1);
    let cache: MirrorCache = serde_json::from_str(&env.state(STATE_MIRRORS).unwrap()).unwrap();
    let mut hosts = cache.hosts;
    hosts.sort();
    assert_eq!(
        hosts,
        vec!["de1.api.radio-browser.info", "fr1.api.radio-browser.info"]
    );
    assert!(now_ms() - cache.at < 60_000);
    assert_eq!(cached_mirrors(&env.db).len(), 2);
}

#[test]
fn titles_emit_and_reset() {
    let env = Env::start(vec![FakeStation::new(1), FakeStation::new(2)]);
    let first = found(&env, 1);
    let sink = (env.service.title_sinks())(&first.id);
    // No station yet: nothing to attach a title to.
    sink(Some("Nobody - Nothing".into()));
    assert!(env.events.lock().unwrap().is_empty());
    assert_eq!(env.service.now_playing(), None);

    let one = env.service.play_prepared(first).unwrap();
    sink(None);
    sink(Some("A - One".into()));
    sink(Some("A - One".into()));
    sink(Some("   ".into()));
    let expected = |title: Option<&str>| {
        RadioEvent::NowPlaying(NowPlaying {
            station_id: one.clone(),
            title: title.map(str::to_owned),
        })
    };
    assert_eq!(
        env.events.lock().unwrap().as_slice(),
        [expected(Some("A - One")), expected(None)]
    );
    assert_eq!(
        env.titles.lock().unwrap().as_slice(),
        [Some("A - One".to_owned()), None]
    );
    sink(Some("B - Two".into()));
    assert_eq!(
        env.service.now_playing(),
        Some(NowPlaying {
            station_id: one.clone(),
            title: Some("B - Two".into())
        })
    );

    // The opener looks the next station up: the old title does not carry over.
    let two = found(&env, 2);
    env.service.set_favorite(two.clone(), true).unwrap();
    let lookup = env.service.lookup();
    assert_eq!(lookup(&two.id).unwrap().id, two.id);
    assert_eq!(
        env.service.now_playing(),
        Some(NowPlaying {
            station_id: two.id.clone(),
            title: None
        })
    );
    // The first station's stream is still closing: its title changes nothing.
    let events = env.events.lock().unwrap().len();
    sink(Some("Late - Title".into()));
    assert_eq!(env.events.lock().unwrap().len(), events);
    assert_eq!(env.service.now_playing().unwrap().title, None);

    env.service.title_sink_for(&two.id)(Some("C - Three".into()));
    assert_eq!(
        env.events.lock().unwrap().last(),
        Some(&RadioEvent::NowPlaying(NowPlaying {
            station_id: two.id,
            title: Some("C - Three".into())
        }))
    );
    assert_eq!(
        env.titles.lock().unwrap().last(),
        Some(&Some("C - Three".to_owned()))
    );
}

#[test]
fn a_pruned_station_is_not_brought_back_by_a_refresh() {
    /// Prunes every station while the directory "answers".
    struct Pruning {
        inner: Directory,
        db: Arc<Db>,
    }
    impl DirectoryApi for Pruning {
        fn search(&self, query: &StationQuery) -> Result<Vec<Station>, DirectoryError> {
            self.inner.search(query)
        }
        fn by_uuids(&self, uuids: &[&str]) -> Result<Vec<Station>, DirectoryError> {
            let conn = self.db.0.lock().unwrap();
            conn.execute("DELETE FROM radio_stations", []).unwrap();
            drop(conn);
            self.inner.by_uuids(uuids)
        }
        fn countries(&self) -> Result<Vec<Country>, DirectoryError> {
            self.inner.countries()
        }
        fn states(&self, name: &str) -> Result<Vec<Region>, DirectoryError> {
            self.inner.states(name)
        }
        fn top_tags(&self, limit: u32) -> Result<Vec<TagCount>, DirectoryError> {
            self.inner.top_tags(limit)
        }
        fn click(&self, uuid: &str) -> Result<(), DirectoryError> {
            self.inner.click(uuid)
        }
        fn refresh_mirrors(&self) -> Result<Vec<String>, DirectoryError> {
            self.inner.refresh_mirrors()
        }
    }

    let fake = FakeDirectory::start(vec![FakeStation::new(1)]);
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let service = RadioService::spawn_with_options(
        Arc::clone(&db),
        Arc::new(SettingsStore::load(&dir.path().join("settings.json"))),
        Arc::new(Pruning {
            inner: Directory::new(MirrorSource::Fixed(vec![fake.url()])).unwrap(),
            db: Arc::clone(&db),
        }),
        Arc::new(FakeLocator {
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(true),
        }),
        Arc::new(|| "de-DE".to_owned()),
        Arc::new(|_| {}),
        Arc::new(|_| {}),
        RadioOptions {
            first_refresh: Duration::from_secs(3600),
            ..RadioOptions::default()
        },
    );
    let mut station = service.search(StationQuery::default()).unwrap().remove(0);
    station.name = "Old Name".into();
    let id = service.play_prepared(station).unwrap();
    // The directory knows a changed station, but it was pruned meanwhile.
    assert_eq!(service.refresh_stations(), Ok(0));
    let conn = db.0.lock().unwrap();
    assert_eq!(store::station(&conn, &id).unwrap(), None);
}

#[test]
fn the_locale_is_read_only_as_the_last_resort() {
    let asked = Arc::new(AtomicUsize::new(0));
    let fake = FakeDirectory::start(Vec::new());
    let directory = Directory::new(MirrorSource::Fixed(vec![fake.url()])).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let locator = Arc::new(FakeLocator {
        calls: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    });
    let counted = Arc::clone(&asked);
    let service = RadioService::spawn(
        Arc::new(Db::open_in_memory().unwrap()),
        Arc::new(SettingsStore::load(&dir.path().join("settings.json"))),
        Arc::new(directory),
        Arc::clone(&locator) as Arc<dyn Locator>,
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            "de-AT".to_owned()
        }),
        Arc::new(|_| {}),
        Arc::new(|_| {}),
    );
    assert_eq!(service.place().origin, PlaceOrigin::Detected);
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    locator.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        service.redetect().origin,
        PlaceOrigin::Detected,
        "the stored place stays"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn near_lists_of_three_places_stay_cached() {
    let env = Env::start(around_testhausen());
    for country in ["DE", "AT", "CH", "FR"] {
        env.manual(country, None);
        env.service.near(true);
        std::thread::sleep(Duration::from_millis(5));
    }
    let conn = env.db.0.lock().unwrap();
    let keys: Vec<String> = conn
        .prepare("SELECT key FROM radio_state WHERE key LIKE 'near:%' ORDER BY key")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(keys, vec!["near:AT||-", "near:CH||-", "near:FR||-"]);
}

#[test]
fn lists_are_asked_once_per_session() {
    let env = Env::start(Vec::new());
    let countries = env.service.countries().unwrap();
    assert!(countries.iter().any(|c| c.code == "DE"));
    env.service.countries().unwrap();
    let regions = env.service.regions("de").unwrap();
    assert!(regions.iter().any(|r| r.name == "Testland"));
    env.service.regions("DE").unwrap();
    assert!(env.service.regions("ZZ").unwrap().is_empty());
    env.service.top_tags().unwrap();
    env.service.top_tags().unwrap();
    assert_eq!(env.requests("/json/countries").len(), 1);
    assert_eq!(env.requests("/json/states").len(), 1);
    assert_eq!(env.requests("/json/tags").len(), 1);
    assert_eq!(
        env.requests("/json/states")[0].param("country"),
        Some("Germany")
    );
}

#[test]
fn search_reports_a_down_directory() {
    let mut env = Env::start(Vec::new());
    env.fake.stop();
    let err = env.service.search(StationQuery::default()).unwrap_err();
    assert_eq!(err.code(), "unreachable");
    let none = Env::with_directory(
        FakeDirectory::start(Vec::new()),
        Arc::new(NoDirectory),
        RadioOptions::default(),
    );
    assert_eq!(
        none.service
            .search(StationQuery::default())
            .unwrap_err()
            .code(),
        "unreachable"
    );
}

#[test]
fn search_results_remember_their_logos() {
    let env = Env::start(vec![
        FakeStation::new(1).favicon("https://www.example.org/demo1/logo.png"),
        FakeStation::new(2).homepage("https://www.example.org/demo2"),
    ]);
    for n in [1, 2] {
        let station = found(&env, n);
        let cover = station.cover_id.clone().unwrap();
        let conn = env.db.0.lock().unwrap();
        assert!(
            store::favicon_source(&conn, &cover).unwrap().is_some(),
            "{n}"
        );
    }
}
