//! A coarse location for "Radio near you": an IP lookup (GeoJS, ipinfo as fallback) or the
//! country and region the user chose.
//!
//! Privacy: coordinates are rounded to one decimal (about 10 km) before they leave
//! [`Locator::locate`], and no error text or log line carries a coordinate or an IP address.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::RadioLocation;

const GEOJS_URL: &str = "https://get.geojs.io/v1/ip/geo.json";
const IPINFO_URL: &str = "https://ipinfo.io/json";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
/// A detected place is asked again after a week.
const DETECTED_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// The country when neither a lookup nor the locale names one.
const FALLBACK_COUNTRY: &str = "US";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceOrigin {
    Detected,
    Manual,
    Locale,
}

/// Where the user is, as coarsely as the radio view needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    /// Two uppercase letters.
    pub country_code: String,
    pub region: Option<String>,
    pub city: Option<String>,
    /// Rounded to one decimal.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub origin: PlaceOrigin,
    /// When a lookup found it (ms since the epoch).
    pub detected_at: Option<i64>,
}

/// Why a lookup gave nothing. The texts name no coordinates, address or host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocateError {
    /// No answer: offline, timed out, or the service failed.
    Unreachable,
    /// An answer without a usable country.
    Unusable,
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocateError::Unreachable => f.write_str("the location service did not answer"),
            LocateError::Unusable => f.write_str("the location service sent no usable place"),
        }
    }
}

impl std::error::Error for LocateError {}

pub trait Locator: Send + Sync {
    fn locate(&self) -> Result<Place, LocateError>;
}

/// Asks GeoJS and, when that fails, ipinfo.
pub struct IpLocator {
    geojs: String,
    ipinfo: String,
}

impl IpLocator {
    pub fn public() -> IpLocator {
        IpLocator::at(GEOJS_URL, IPINFO_URL)
    }

    pub fn at(geojs: impl Into<String>, ipinfo: impl Into<String>) -> IpLocator {
        IpLocator {
            geojs: geojs.into(),
            ipinfo: ipinfo.into(),
        }
    }

    fn fetch(url: &str) -> Result<String, LocateError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("Prism/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(LOOKUP_TIMEOUT)
            .timeout(LOOKUP_TIMEOUT)
            .build()
            .map_err(|_| LocateError::Unreachable)?;
        let response = client
            .get(url)
            .header("Accept", "application/json")
            .send()
            .map_err(|_| LocateError::Unreachable)?;
        if !response.status().is_success() {
            return Err(LocateError::Unreachable);
        }
        // Both answers are a few hundred bytes; a bigger body is not one of them.
        let bytes = response.bytes().map_err(|_| LocateError::Unreachable)?;
        if bytes.len() > 64 * 1024 {
            return Err(LocateError::Unusable);
        }
        String::from_utf8(bytes.to_vec()).map_err(|_| LocateError::Unusable)
    }
}

impl Locator for IpLocator {
    fn locate(&self) -> Result<Place, LocateError> {
        let first = IpLocator::fetch(&self.geojs).and_then(|body| parse_geojs(&body));
        match first {
            Ok(place) => Ok(place),
            Err(_) => IpLocator::fetch(&self.ipinfo).and_then(|body| parse_ipinfo(&body)),
        }
    }
}

/// A coordinate as a number or a numeric string, rounded to one decimal; out of range is none.
fn coordinate(value: Option<&Value>, limit: f64) -> Option<f64> {
    let number = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !number.is_finite() || number.abs() > limit {
        return None;
    }
    // `+ 0.0` turns -0.0 into 0.0.
    Some((number * 10.0).round() / 10.0 + 0.0)
}

fn text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.chars().take(80).collect())
}

fn country(value: Option<&Value>) -> Option<String> {
    let code = value?.as_str()?.trim();
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| code.to_ascii_uppercase())
}

fn detected(
    country_code: String,
    region: Option<String>,
    city: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
) -> Place {
    // One coordinate alone is no position.
    let (lat, lon) = match (lat, lon) {
        (Some(lat), Some(lon)) => (Some(lat), Some(lon)),
        _ => (None, None),
    };
    Place {
        country_code,
        region,
        city,
        lat,
        lon,
        origin: PlaceOrigin::Detected,
        detected_at: None,
    }
}

fn parse_geojs(body: &str) -> Result<Place, LocateError> {
    let json: Value = serde_json::from_str(body).map_err(|_| LocateError::Unusable)?;
    let country_code = country(json.get("country_code")).ok_or(LocateError::Unusable)?;
    Ok(detected(
        country_code,
        text(json.get("region")),
        text(json.get("city")),
        coordinate(json.get("latitude"), 90.0),
        coordinate(json.get("longitude"), 180.0),
    ))
}

fn parse_ipinfo(body: &str) -> Result<Place, LocateError> {
    let json: Value = serde_json::from_str(body).map_err(|_| LocateError::Unusable)?;
    let country_code = country(json.get("country")).ok_or(LocateError::Unusable)?;
    let (lat, lon) = json
        .get("loc")
        .and_then(Value::as_str)
        .and_then(|loc| loc.split_once(','))
        .map(|(lat, lon)| {
            (
                coordinate(Some(&Value::String(lat.to_owned())), 90.0),
                coordinate(Some(&Value::String(lon.to_owned())), 180.0),
            )
        })
        .unwrap_or((None, None));
    Ok(detected(
        country_code,
        text(json.get("region")),
        text(json.get("city")),
        lat,
        lon,
    ))
}

/// The country of a system locale: `de-DE` and `de_DE.UTF-8` give `DE`, `en` gives none.
pub fn locale_country(locale: &str) -> Option<String> {
    let tag = locale.split(['.', '@']).next().unwrap_or("");
    tag.split(['-', '_'])
        .skip(1)
        .find(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()))
        .map(|part| part.to_ascii_uppercase())
}

/// The place the radio view uses. Only an automatic lookup that succeeded is worth storing
/// (the second value); manual mode never calls `locator`.
pub fn resolve_place(
    setting: &RadioLocation,
    cached: Option<Place>,
    locator: &dyn Locator,
    locale: &str,
    now_ms: i64,
) -> (Place, bool) {
    if let RadioLocation::Manual {
        country_code,
        region,
    } = setting
    {
        let region = region
            .as_deref()
            .map(str::trim)
            .filter(|region| !region.is_empty())
            .map(str::to_owned);
        return (
            Place {
                country_code: country_code.to_ascii_uppercase(),
                region,
                city: None,
                lat: None,
                lon: None,
                origin: PlaceOrigin::Manual,
                detected_at: None,
            },
            false,
        );
    }
    let cached = cached.filter(|place| place.origin == PlaceOrigin::Detected);
    if let Some(place) = &cached {
        let age = place.detected_at.map(|at| now_ms - at);
        if age.is_some_and(|age| (0..DETECTED_TTL_MS).contains(&age)) {
            return (place.clone(), false);
        }
    }
    match locator.locate() {
        Ok(mut place) => {
            place.origin = PlaceOrigin::Detected;
            place.detected_at = Some(now_ms);
            (place, true)
        }
        Err(_) => match cached {
            Some(place) => (place, false),
            None => (
                Place {
                    country_code: locale_country(locale)
                        .unwrap_or_else(|| FALLBACK_COUNTRY.to_owned()),
                    region: None,
                    city: None,
                    lat: None,
                    lon: None,
                    origin: PlaceOrigin::Locale,
                    detected_at: None,
                },
                false,
            ),
        },
    }
}

/// The JSON that goes into `radio_state` key `place`.
pub fn place_to_json(place: &Place) -> String {
    serde_json::to_string(place).unwrap_or_else(|_| "null".to_owned())
}

/// Reads a stored place; anything unreadable is none, so the next lookup replaces it.
pub fn place_from_json(json: &str) -> Option<Place> {
    let place: Place = serde_json::from_str(json).ok()?;
    let valid =
        place.country_code.len() == 2 && place.country_code.bytes().all(|b| b.is_ascii_uppercase());
    valid.then_some(place)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::testserver::FakeGeo;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn geojs_fixture() -> &'static str {
        include_str!("../../tests/fixtures/radio/geojs.json")
    }

    fn testhausen(detected_at: i64) -> Place {
        Place {
            country_code: "DE".into(),
            region: Some("Testland".into()),
            city: Some("Testhausen".into()),
            lat: Some(50.0),
            lon: Some(10.0),
            origin: PlaceOrigin::Detected,
            detected_at: Some(detected_at),
        }
    }

    struct PanicLocator;
    impl Locator for PanicLocator {
        fn locate(&self) -> Result<Place, LocateError> {
            panic!("the service must not be called");
        }
    }

    struct FixedLocator(Result<Place, LocateError>);
    impl Locator for FixedLocator {
        fn locate(&self) -> Result<Place, LocateError> {
            self.0.clone()
        }
    }

    #[test]
    fn geojs_answer_parses() {
        let geo = FakeGeo::start();
        let place = IpLocator::at(geo.geojs_url(), geo.ipinfo_url())
            .locate()
            .unwrap();
        assert_eq!(place.country_code, "DE");
        assert_eq!(place.region.as_deref(), Some("Testland"));
        assert_eq!(place.city.as_deref(), Some("Testhausen"));
        assert_eq!(place.origin, PlaceOrigin::Detected);
        assert_eq!(geo.geojs_hits(), 1);
        assert_eq!(geo.ipinfo_hits(), 0, "the fallback was not needed");
        // Numbers work as well as the strings GeoJS really sends.
        let numbers = parse_geojs(r#"{"country_code":"de","latitude":50.0,"longitude":10}"#);
        assert_eq!(numbers.unwrap().lat, Some(50.0));
    }

    #[test]
    fn ipinfo_is_the_fallback() {
        let geo = FakeGeo::start();
        geo.set_geojs(500, "{}");
        let place = IpLocator::at(geo.geojs_url(), geo.ipinfo_url())
            .locate()
            .unwrap();
        assert_eq!(place.country_code, "DE");
        assert_eq!(place.city.as_deref(), Some("Testhausen"));
        assert_eq!((place.lat, place.lon), (Some(50.0), Some(10.1)));
        assert_eq!((geo.geojs_hits(), geo.ipinfo_hits()), (1, 1));
        // An answer without a country counts as a failure, too.
        geo.set_geojs(200, r#"{"country_code":"","latitude":"1","longitude":"2"}"#);
        let place = IpLocator::at(geo.geojs_url(), geo.ipinfo_url())
            .locate()
            .unwrap();
        assert_eq!(place.country_code, "DE");
        assert_eq!(geo.ipinfo_hits(), 2);
        // Both down: an error.
        geo.set_ipinfo(503, "{}");
        let err = IpLocator::at(geo.geojs_url(), geo.ipinfo_url())
            .locate()
            .unwrap_err();
        assert_eq!(err, LocateError::Unreachable);
    }

    #[test]
    fn detected_location_is_rounded() {
        let geo = FakeGeo::start();
        let locator = IpLocator::at(geo.geojs_url(), geo.ipinfo_url());
        let place = locator.locate().unwrap();
        assert_eq!((place.lat, place.lon), (Some(50.0), Some(10.1)));
        geo.set_geojs(500, "{}");
        let place = locator.locate().unwrap();
        assert_eq!((place.lat, place.lon), (Some(50.0), Some(10.1)));
        assert_eq!(coordinate(Some(&Value::from(-0.04)), 90.0), Some(0.0));
        assert!(coordinate(Some(&Value::from("95.5")), 90.0).is_none());
        let half = parse_geojs(r#"{"country_code":"DE","latitude":"50.1"}"#).unwrap();
        assert_eq!(
            (half.lat, half.lon),
            (None, None),
            "one coordinate alone is none"
        );
        let stored = place_to_json(&place);
        assert!(!stored.contains("50.04321") && !stored.contains("10.0987"));
    }

    #[test]
    fn manual_mode_never_calls_the_service() {
        let setting = RadioLocation::Manual {
            country_code: "DE".into(),
            region: Some(" Testland ".into()),
        };
        let cached = Some(testhausen(NOW));
        let (place, store) = resolve_place(&setting, cached, &PanicLocator, "en-US", NOW);
        assert!(!store);
        assert_eq!(place.origin, PlaceOrigin::Manual);
        assert_eq!(place.country_code, "DE");
        assert_eq!(place.region.as_deref(), Some("Testland"));
        assert_eq!((place.lat, place.lon, place.city), (None, None, None));
    }

    #[test]
    fn fresh_cache_is_used() {
        let cached = testhausen(NOW - 6 * DAY);
        let (place, store) = resolve_place(
            &RadioLocation::Auto,
            Some(cached.clone()),
            &PanicLocator,
            "en-US",
            NOW,
        );
        assert_eq!(place, cached);
        assert!(!store);
    }

    #[test]
    fn stale_cache_triggers_a_lookup() {
        let mut fresh = testhausen(0);
        fresh.city = Some("Newtown".into());
        for age in [7 * DAY, 30 * DAY, -DAY] {
            let (place, store) = resolve_place(
                &RadioLocation::Auto,
                Some(testhausen(NOW - age)),
                &FixedLocator(Ok(fresh.clone())),
                "en-US",
                NOW,
            );
            assert_eq!(place.city.as_deref(), Some("Newtown"), "age {age}");
            assert_eq!(place.detected_at, Some(NOW));
            assert!(store);
        }
        let (place, store) = resolve_place(
            &RadioLocation::Auto,
            None,
            &FixedLocator(Ok(fresh)),
            "en-US",
            NOW,
        );
        assert!(store);
        assert_eq!(place.origin, PlaceOrigin::Detected);
    }

    #[test]
    fn failed_lookup_keeps_the_old_place() {
        let old = testhausen(NOW - 90 * DAY);
        let (place, store) = resolve_place(
            &RadioLocation::Auto,
            Some(old.clone()),
            &FixedLocator(Err(LocateError::Unreachable)),
            "en-US",
            NOW,
        );
        assert_eq!(place, old);
        assert!(!store, "a failed lookup stores nothing");
    }

    #[test]
    fn locale_is_the_last_resort() {
        let failing = FixedLocator(Err(LocateError::Unreachable));
        let (place, store) = resolve_place(&RadioLocation::Auto, None, &failing, "de-DE", NOW);
        assert_eq!(place.country_code, "DE");
        assert_eq!(place.origin, PlaceOrigin::Locale);
        assert_eq!((place.lat, place.lon, place.region), (None, None, None));
        assert!(!store);
        let (place, _) = resolve_place(&RadioLocation::Auto, None, &failing, "en", NOW);
        assert_eq!(place.country_code, "US");
        assert_eq!(locale_country("de_AT.UTF-8").as_deref(), Some("AT"));
        assert_eq!(locale_country("zh-Hans-cn").as_deref(), Some("CN"));
        assert_eq!(locale_country("de"), None);
        assert_eq!(locale_country(""), None);
        // A manual or locale place in the cache is not a detection and is not reused.
        let mut manual = testhausen(NOW);
        manual.origin = PlaceOrigin::Manual;
        let (place, _) = resolve_place(&RadioLocation::Auto, Some(manual), &failing, "fr-FR", NOW);
        assert_eq!(place.country_code, "FR");
    }

    #[test]
    fn errors_carry_no_coordinates() {
        // A closed port and broken bodies: the errors say nothing about the place.
        let dead = IpLocator::at("http://127.0.0.1:1/geo", "http://127.0.0.1:1/json");
        let mut texts = vec![dead.locate().unwrap_err().to_string()];
        for body in [
            geojs_fixture().replace("DE\"", "Germany\""),
            "<html>".into(),
        ] {
            texts.push(parse_geojs(&body).unwrap_err().to_string());
            texts.push(format!("{:?}", parse_geojs(&body).unwrap_err()));
        }
        for text in texts {
            for secret in ["50.", "10.0", "192.0.2.1", "Testhausen", "127.0.0.1"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    #[test]
    fn stored_place_round_trips_and_garbage_is_none() {
        let place = testhausen(NOW);
        assert_eq!(place_from_json(&place_to_json(&place)), Some(place));
        assert_eq!(place_from_json("null"), None);
        assert_eq!(place_from_json("{"), None);
        assert_eq!(
            place_from_json(
                r#"{"countryCode":"de","origin":"detected","region":null,"city":null,"lat":null,"lon":null,"detectedAt":null}"#
            ),
            None
        );
    }
}
