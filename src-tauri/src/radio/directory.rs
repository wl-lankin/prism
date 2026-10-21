//! The Radio Browser client: station search, lookups by uuid, countries, regions, tags and the
//! play click, with rotation over the directory's mirrors.
//!
//! Everything the directory sends is untrusted: numbers may be strings or null, names may carry
//! control characters, bodies may be huge or HTML. Errors name the mirror's host and nothing else
//! (never the path or the query).

use std::fmt;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::bail;
use rand::seq::SliceRandom;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::radio::model::{clean_name, clean_tags, station_cover_id, station_id, Station};

pub const USER_AGENT: &str = concat!("Prism/", env!("CARGO_PKG_VERSION"));
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
pub const API_TIMEOUT: Duration = Duration::from_secs(12);
/// A discovered mirror list is kept this long before it is asked for again.
pub const MIRROR_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const FALLBACK_MIRROR: &str = "de1.api.radio-browser.info";
pub const MAX_RESPONSE: usize = 8 * 1024 * 1024;
pub const MAX_LIMIT: u32 = 500;
const DISCOVERY_BASE: &str = "https://all.api.radio-browser.info";
const UUID_BATCH: usize = 50;
const MAX_URL_BYTES: usize = 2048;
const MAX_TEXT_CHARS: usize = 60;
const MAX_MIRRORS: usize = 16;
const VIDEO_CODECS: [&str; 6] = ["H.264", "H264", "H.265", "H265", "VP8", "VP9"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryError {
    /// No mirror answered (offline, refused, timed out). Carries the last mirror's host.
    Unreachable(String),
    /// A mirror answered with this status.
    Server(u16),
    /// A mirror answered with something that is not what was asked for.
    BadResponse(String),
}

impl DirectoryError {
    pub fn code(&self) -> &'static str {
        match self {
            DirectoryError::Unreachable(_) => "unreachable",
            DirectoryError::Server(_) => "server",
            DirectoryError::BadResponse(_) => "badResponse",
        }
    }

    /// The directory could not be reached at all.
    pub fn is_offline(&self) -> bool {
        matches!(self, DirectoryError::Unreachable(_))
    }

    /// Offline, or every mirror failed on its side (5xx): "near you" shows its cache.
    pub fn is_down(&self) -> bool {
        match self {
            DirectoryError::Unreachable(_) => true,
            DirectoryError::Server(status) => *status >= 500,
            DirectoryError::BadResponse(_) => false,
        }
    }
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectoryError::Unreachable(host) => write!(f, "{host} did not answer"),
            DirectoryError::Server(status) => write!(f, "the directory answered {status}"),
            DirectoryError::BadResponse(what) => write!(f, "the directory sent {what}"),
        }
    }
}

impl std::error::Error for DirectoryError {}

// --- lenient fields ---------------------------------------------------------------------------

fn value_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && f.abs() < 9e15)
                .map(|f| f as i64)
        }),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn value_f64(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

fn lenient_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(value_i64(&Value::deserialize(d)?))
}

fn lenient_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(value_f64(&Value::deserialize(d)?))
}

fn lenient_flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::String(s) => s.trim() == "1" || s.trim().eq_ignore_ascii_case("true"),
        other => value_i64(&other).is_some_and(|n| n != 0),
    })
}

// --- stations ---------------------------------------------------------------------------------

/// A station as Radio Browser sends it. Every field may be missing, null or oddly typed.
#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)] // the fields the cleaned `Station` does not keep still document the wire shape
pub(crate) struct RawStation {
    #[serde(default, deserialize_with = "lenient_text")]
    pub stationuuid: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub url_resolved: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub homepage: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub favicon: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub tags: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub countrycode: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub state: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub language: Option<String>,
    #[serde(default, deserialize_with = "lenient_text")]
    pub codec: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub bitrate: Option<i64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    pub hls: bool,
    #[serde(default, deserialize_with = "lenient_flag")]
    pub lastcheckok: bool,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub clickcount: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub votes: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub geo_lat: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub geo_long: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub geo_distance: Option<f64>,
}

/// Control characters removed, whitespace collapsed, at most `max` characters. `None` if empty.
pub(crate) fn clean_text(raw: &str, max: usize) -> Option<String> {
    let spaced: String = raw
        .chars()
        .filter_map(|ch| {
            if ch.is_whitespace() {
                Some(' ')
            } else if ch.is_control() {
                None
            } else {
                Some(ch)
            }
        })
        .collect();
    let joined = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(max).collect();
    (!cut.is_empty()).then_some(cut)
}

/// An http(s) URL without whitespace or control characters, at most 2 048 bytes.
fn http_url(raw: Option<&str>) -> Option<String> {
    let url = raw?.trim();
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let schemed = lower.starts_with("http://") || lower.starts_with("https://");
    let clean = !url.chars().any(|ch| ch.is_control() || ch.is_whitespace());
    (schemed && clean && url.len() <= MAX_URL_BYTES && url.len() > 8).then(|| url.to_owned())
}

fn is_video_codec(codec: &str) -> bool {
    let upper = codec.to_ascii_uppercase();
    VIDEO_CODECS.iter().any(|video| upper.contains(video))
}

impl RawStation {
    /// The cleaned station, or `None` for one the player must never see: a bad uuid, a name
    /// that is empty after cleanup, a video codec, a stream that is not http(s).
    pub(crate) fn into_station(self) -> Option<Station> {
        let id = station_id(self.stationuuid.as_deref()?.trim())?;
        let name = clean_name(self.name.as_deref()?)?;
        let url = http_url(self.url.as_deref())?;
        let codec_raw = self.codec.as_deref().unwrap_or_default();
        if is_video_codec(codec_raw) {
            return None;
        }
        let codec = clean_text(codec_raw, 24).filter(|c| !c.eq_ignore_ascii_case("unknown"));
        let favicon = http_url(self.favicon.as_deref());
        let homepage = http_url(self.homepage.as_deref());
        let country_code = self
            .countrycode
            .as_deref()
            .map(str::trim)
            .filter(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()))
            .map(str::to_ascii_uppercase);
        Some(Station {
            id,
            name,
            url,
            url_resolved: http_url(self.url_resolved.as_deref()),
            cover_id: station_cover_id(favicon.as_deref(), homepage.as_deref()),
            homepage,
            favicon,
            country_code,
            region: self
                .state
                .as_deref()
                .and_then(|state| clean_text(state, MAX_TEXT_CHARS)),
            language: self
                .language
                .as_deref()
                .and_then(|language| clean_text(language, MAX_TEXT_CHARS)),
            tags: clean_tags(self.tags.as_deref().unwrap_or_default()),
            codec,
            bitrate_kbps: self
                .bitrate
                .and_then(|rate| u32::try_from(rate).ok())
                .filter(|rate| *rate > 0),
            hls: self.hls,
            last_check_ok: self.lastcheckok,
            distance_km: self
                .geo_distance
                .filter(|metres| *metres >= 0.0)
                .map(|metres| (metres / 1000.0) as f32),
        })
    }
}

/// Parses a station array element by element: an element that is not a station is skipped.
fn parse_stations(body: &[u8], host: &str) -> Result<Vec<Station>, DirectoryError> {
    let items = parse_array(body, host)?;
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<RawStation>(item).ok())
        .filter_map(RawStation::into_station)
        .collect())
}

fn parse_array(body: &[u8], host: &str) -> Result<Vec<Value>, DirectoryError> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Array(items)) => Ok(items),
        _ => Err(DirectoryError::BadResponse(format!(
            "something other than a list ({host})"
        ))),
    }
}

// --- queries and lists ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    #[default]
    ClickCount,
    Votes,
    Name,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StationQuery {
    pub name: Option<String>,
    pub country_code: Option<String>,
    pub region: Option<String>,
    pub tag: Option<String>,
    /// Latitude, longitude, radius in metres.
    pub geo: Option<(f64, f64, f64)>,
    pub order: Order,
    pub offset: u32,
    pub limit: u32,
}

impl Default for StationQuery {
    fn default() -> Self {
        StationQuery {
            name: None,
            country_code: None,
            region: None,
            tag: None,
            geo: None,
            order: Order::default(),
            offset: 0,
            limit: 50,
        }
    }
}

impl StationQuery {
    fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![("hidebroken", "true".to_owned())];
        if let Some(name) = self.name.as_deref().filter(|n| !n.trim().is_empty()) {
            params.push(("name", name.trim().to_owned()));
        }
        if let Some(code) = self.country_code.as_deref().filter(|c| !c.is_empty()) {
            params.push(("countrycode", code.to_ascii_uppercase()));
        }
        if let Some(region) = self.region.as_deref().filter(|r| !r.is_empty()) {
            params.push(("state", region.to_owned()));
            params.push(("stateExact", "true".to_owned()));
        }
        if let Some(tag) = self.tag.as_deref().filter(|t| !t.is_empty()) {
            params.push(("tag", tag.to_owned()));
        }
        if let Some((lat, lon, metres)) = self.geo {
            params.push(("has_geo_info", "true".to_owned()));
            params.push(("geo_lat", lat.to_string()));
            params.push(("geo_long", lon.to_string()));
            params.push(("geo_distance", metres.to_string()));
        }
        let (order, reverse) = match self.order {
            Order::ClickCount => ("clickcount", true),
            Order::Votes => ("votes", true),
            Order::Name => ("name", false),
        };
        params.push(("order", order.to_owned()));
        params.push(("reverse", reverse.to_string()));
        params.push(("offset", self.offset.to_string()));
        params.push(("limit", self.limit.min(MAX_LIMIT).to_string()));
        params
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Country {
    pub code: String,
    pub name: String,
    pub station_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub name: String,
    pub station_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub name: String,
    pub station_count: u32,
}

fn field_text(item: &Value, key: &str, max: usize) -> Option<String> {
    clean_text(item.get(key)?.as_str()?, max)
}

fn field_count(item: &Value, key: &str) -> u32 {
    item.get(key)
        .and_then(value_i64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

// --- mirrors ----------------------------------------------------------------------------------

pub enum MirrorSource {
    /// Host names from the cache (7 days); empty falls back to [`FALLBACK_MIRROR`]. Shuffled once.
    Discover { cached: Vec<String> },
    /// Base URLs with scheme, tried in this order (tests).
    Fixed(Vec<String>),
}

struct Mirrors {
    bases: Vec<String>,
    /// Index of the mirror that answered last.
    first: usize,
}

fn valid_host(name: &str) -> bool {
    name.len() <= 253
        && name.contains('.')
        && name
            .split('.')
            .all(|label| !label.is_empty() && label.len() <= 63)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

fn host_bases(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim().to_ascii_lowercase();
        if valid_host(&name) && !seen.contains(&name) && seen.len() < MAX_MIRRORS {
            seen.push(name);
        }
    }
    seen.shuffle(&mut rand::rng());
    seen.into_iter()
        .map(|name| format!("https://{name}"))
        .collect()
}

fn host_of(base: &str) -> String {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the directory".to_owned())
}

pub struct Directory {
    client: reqwest::blocking::Client,
    mirrors: Mutex<Mirrors>,
    discovery_base: String,
}

impl Directory {
    pub fn new(source: MirrorSource) -> anyhow::Result<Directory> {
        Directory::with_timeouts(source, CONNECT_TIMEOUT, API_TIMEOUT)
    }

    pub(crate) fn with_timeouts(
        source: MirrorSource,
        connect: Duration,
        total: Duration,
    ) -> anyhow::Result<Directory> {
        let bases = match source {
            MirrorSource::Discover { cached } => {
                let bases = host_bases(cached);
                if bases.is_empty() {
                    vec![format!("https://{FALLBACK_MIRROR}")]
                } else {
                    bases
                }
            }
            MirrorSource::Fixed(urls) => urls
                .into_iter()
                .map(|url| url.trim_end_matches('/').to_owned())
                .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
                .collect(),
        };
        if bases.is_empty() {
            bail!("no directory mirror to ask");
        }
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(connect)
            .timeout(total)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()?;
        Ok(Directory {
            client,
            mirrors: Mutex::new(Mirrors { bases, first: 0 }),
            discovery_base: DISCOVERY_BASE.to_owned(),
        })
    }

    #[cfg(test)]
    pub(crate) fn set_discovery_base(&mut self, base: &str) {
        self.discovery_base = base.trim_end_matches('/').to_owned();
    }

    /// The mirror host names in use, for the 7-day cache.
    pub fn mirrors(&self) -> Vec<String> {
        let mirrors = self.mirrors.lock().unwrap_or_else(|e| e.into_inner());
        mirrors.bases.iter().map(|base| host_of(base)).collect()
    }

    /// Asks the discovery host for the mirror list (deduplicated by name, invalid names dropped)
    /// and switches to it. Returns the names for the cache. An empty answer keeps the old list.
    pub fn refresh_mirrors(&self) -> Result<Vec<String>, DirectoryError> {
        let host = host_of(&self.discovery_base);
        let body = self.fetch(&self.discovery_base, "/json/servers", &[])?;
        let names = parse_array(&body, &host)?
            .iter()
            .filter_map(|item| item.get("name")?.as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let bases = host_bases(names);
        if bases.is_empty() {
            return Err(DirectoryError::BadResponse(format!("no mirrors ({host})")));
        }
        let names = bases.iter().map(|base| host_of(base)).collect();
        let mut mirrors = self.mirrors.lock().unwrap_or_else(|e| e.into_inner());
        *mirrors = Mirrors { bases, first: 0 };
        Ok(names)
    }

    /// One GET against one base; the body is read with a cap.
    fn fetch(
        &self,
        base: &str,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Vec<u8>, DirectoryError> {
        #[cfg(test)]
        crate::radio::testserver::assert_loopback(base);
        let host = host_of(base);
        let response = self
            .client
            .get(format!("{base}{path}"))
            .query(params)
            .header("Accept", "application/json")
            .send()
            .map_err(|_| DirectoryError::Unreachable(host.clone()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(DirectoryError::Server(status.as_u16()));
        }
        let too_big = || DirectoryError::BadResponse(format!("more than 8 MiB ({host})"));
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE as u64)
        {
            return Err(too_big());
        }
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|_| DirectoryError::Unreachable(host.clone()))?;
        if body.len() > MAX_RESPONSE {
            return Err(too_big());
        }
        Ok(body)
    }

    /// Tries each mirror at most once, starting with the one that answered last. Offline and 5xx
    /// move on; anything else ends the request. Returns the body and the host that sent it.
    fn get(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<(Vec<u8>, String), DirectoryError> {
        let (bases, start) = {
            let mirrors = self.mirrors.lock().unwrap_or_else(|e| e.into_inner());
            (mirrors.bases.clone(), mirrors.first)
        };
        let mut last = None;
        for step in 0..bases.len() {
            let index = (start + step) % bases.len();
            match self.fetch(&bases[index], path, params) {
                Ok(body) => {
                    let mut mirrors = self.mirrors.lock().unwrap_or_else(|e| e.into_inner());
                    if mirrors.bases == bases {
                        mirrors.first = index;
                    }
                    return Ok((body, host_of(&bases[index])));
                }
                Err(err) if err.is_down() => last = Some(err),
                Err(err) => return Err(err),
            }
        }
        Err(last.unwrap_or_else(|| DirectoryError::Unreachable("the directory".to_owned())))
    }

    fn get_list(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Vec<Value>, DirectoryError> {
        let (body, host) = self.get(path, params)?;
        parse_array(&body, &host)
    }

    pub fn search(&self, query: &StationQuery) -> Result<Vec<Station>, DirectoryError> {
        let (body, host) = self.get("/json/stations/search", &query.params())?;
        parse_stations(&body, &host)
    }

    /// Stations by uuid (invalid uuids are skipped), asked in batches of 50.
    pub fn by_uuids(&self, uuids: &[&str]) -> Result<Vec<Station>, DirectoryError> {
        let mut wanted: Vec<String> = Vec::new();
        for uuid in uuids {
            if let Some(id) = station_id(uuid.trim()) {
                let uuid = id
                    .trim_start_matches(crate::radio::model::RADIO_PREFIX)
                    .to_owned();
                if !wanted.contains(&uuid) {
                    wanted.push(uuid);
                }
            }
        }
        let mut stations = Vec::new();
        for batch in wanted.chunks(UUID_BATCH) {
            let params = [("uuids", batch.join(","))];
            let (body, host) = self.get("/json/stations/byuuid", &params)?;
            stations.extend(parse_stations(&body, &host)?);
        }
        Ok(stations)
    }

    /// Countries with stations, the biggest first.
    pub fn countries(&self) -> Result<Vec<Country>, DirectoryError> {
        let params = [
            ("order", "stationcount".to_owned()),
            ("reverse", "true".to_owned()),
            ("hidebroken", "true".to_owned()),
        ];
        Ok(self
            .get_list("/json/countries", &params)?
            .iter()
            .filter_map(|item| {
                let code = item.get("iso_3166_1")?.as_str()?.trim();
                if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_alphabetic()) {
                    return None;
                }
                Some(Country {
                    code: code.to_ascii_uppercase(),
                    name: field_text(item, "name", MAX_TEXT_CHARS)?,
                    station_count: field_count(item, "stationcount"),
                })
            })
            .collect())
    }

    /// Regions of a country (by its directory name, e.g. "Germany"), the biggest first.
    pub fn states(&self, country_name: &str) -> Result<Vec<Region>, DirectoryError> {
        let params = [
            ("country", country_name.to_owned()),
            ("order", "stationcount".to_owned()),
            ("reverse", "true".to_owned()),
            ("hidebroken", "true".to_owned()),
        ];
        Ok(self
            .get_list("/json/states", &params)?
            .iter()
            .filter_map(|item| {
                Some(Region {
                    name: field_text(item, "name", MAX_TEXT_CHARS)?,
                    station_count: field_count(item, "stationcount"),
                })
            })
            .collect())
    }

    pub fn top_tags(&self, limit: u32) -> Result<Vec<TagCount>, DirectoryError> {
        let params = [
            ("order", "stationcount".to_owned()),
            ("reverse", "true".to_owned()),
            ("hidebroken", "true".to_owned()),
            ("limit", limit.clamp(1, MAX_LIMIT).to_string()),
        ];
        Ok(self
            .get_list("/json/tags", &params)?
            .iter()
            .filter_map(|item| {
                Some(TagCount {
                    name: field_text(item, "name", 40)?.to_lowercase(),
                    station_count: field_count(item, "stationcount"),
                })
            })
            .collect())
    }

    /// Reports that a station is being played (the directory's click counter).
    pub fn click(&self, uuid: &str) -> Result<(), DirectoryError> {
        let Some(id) = station_id(uuid.trim()) else {
            return Err(DirectoryError::BadResponse("a bad station id".to_owned()));
        };
        let uuid = id.trim_start_matches(crate::radio::model::RADIO_PREFIX);
        self.get(&format!("/json/url/{uuid}"), &[]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::testserver::{FakeDirectory, FakeStation, Fault};

    const SEARCH: &str = include_str!("../../tests/fixtures/radio/search.json");
    const SEARCH_GEO: &str = include_str!("../../tests/fixtures/radio/search_geo.json");
    const BYUUID: &str = include_str!("../../tests/fixtures/radio/byuuid.json");

    fn directory(servers: &[&FakeDirectory]) -> Directory {
        Directory::with_timeouts(
            MirrorSource::Fixed(servers.iter().map(|s| s.url()).collect()),
            Duration::from_secs(2),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    fn raw(json: &str) -> RawStation {
        serde_json::from_str(json).unwrap()
    }

    const GOOD: &str = r#"{"stationuuid":"00000000-0000-4000-8000-000000000001","name":"Demo Radio 1","url":"http://stream.example.org/a"}"#;

    #[test]
    fn fixtures_parse() {
        let stations = parse_stations(SEARCH.as_bytes(), "t").unwrap();
        assert_eq!(stations.len(), 3);
        let first = &stations[0];
        assert_eq!(first.id, "radio:00000000-0000-4000-8000-000000000001");
        assert_eq!(first.name, "CLUB CHARTS TOP 100 || Demo Radio 1");
        assert_eq!(first.tags, ["pop", "rock", "hits"]);
        assert_eq!(first.codec.as_deref(), Some("MP3"));
        assert_eq!(first.bitrate_kbps, Some(128));
        assert_eq!(first.country_code.as_deref(), Some("DE"));
        assert_eq!(first.region.as_deref(), Some("Testland"));
        assert!(first.last_check_ok && !first.hls);
        assert!(first.cover_id.as_deref().unwrap().starts_with("rb-"));
        let second = &stations[1];
        assert_eq!(second.url_resolved, None);
        assert_eq!(second.favicon, None);
        assert_eq!(second.cover_id, None);
        let third = &stations[2];
        assert!(third.hls);
        assert_eq!(third.codec, None);
        assert_eq!(third.bitrate_kbps, None);

        let geo = parse_stations(SEARCH_GEO.as_bytes(), "t").unwrap();
        assert_eq!(geo.len(), 2);
        assert_eq!(geo[0].distance_km, Some(3.4127));
        assert_eq!(geo[1].distance_km, Some(24.8));

        assert_eq!(parse_stations(BYUUID.as_bytes(), "t").unwrap().len(), 1);
    }

    #[test]
    fn lenient_numbers_and_nulls() {
        let station = raw(
            r#"{"stationuuid":"00000000-0000-4000-8000-000000000001","name":"Demo Radio 1",
            "url":"http://stream.example.org/a","bitrate":"128","hls":"1","lastcheckok":1,
            "geo_lat":null,"geo_long":"10.5","geo_distance":"2500","clickcount":null,"votes":"7"}"#,
        );
        assert_eq!(station.geo_lat, None);
        assert_eq!(station.geo_long, Some(10.5));
        assert_eq!(station.votes, Some(7));
        let station = station.into_station().unwrap();
        assert_eq!(station.bitrate_kbps, Some(128));
        assert!(station.hls && station.last_check_ok);
        assert_eq!(station.distance_km, Some(2.5));

        let sparse = raw(
            r#"{"stationuuid":"00000000-0000-4000-8000-000000000001","name":null,"url":"http://a.example.org/x","bitrate":null}"#,
        );
        assert!(sparse.into_station().is_none());
        assert!(raw(GOOD).into_station().is_some());
    }

    #[test]
    fn bad_stations_are_dropped() {
        let base = |name: &str, url: &str, codec: &str, uuid: &str| {
            format!(r#"{{"stationuuid":"{uuid}","name":"{name}","url":"{url}","codec":"{codec}"}}"#)
        };
        let ok = "00000000-0000-4000-8000-000000000001";
        let cases = [
            base("Demo Radio 1", "http://a.example.org/x", "MP3", ok),
            base(" --- ", "http://a.example.org/x", "MP3", ok),
            base("Demo Radio 2", "http://a.example.org/x", "AAC,H.264", ok),
            base("Demo Radio 3", "http://a.example.org/x", "VP9", ok),
            base("Demo Radio 4", "rtsp://a.example.org/x", "MP3", ok),
            base("Demo Radio 5", "http://a.example.org/x", "MP3", "../x"),
            base("Demo Radio 6", "http://a.example.org/ x", "MP3", ok),
            base("Demo Radio 7", "http://", "MP3", ok),
        ];
        let kept: Vec<bool> = cases
            .iter()
            .map(|json| raw(json).into_station().is_some())
            .collect();
        assert_eq!(
            kept,
            [true, false, false, false, false, false, false, false]
        );
        let body = format!("[{}, 5, \"x\", null, {GOOD}]", cases[2]);
        assert_eq!(parse_stations(body.as_bytes(), "t").unwrap().len(), 1);
        assert!(parse_stations(b"{}", "t").is_err());
    }

    #[test]
    fn hostile_text_is_cleaned() {
        let long = "x".repeat(2000);
        let tags = vec!["tag"; 300]
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{t}{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"stationuuid":"00000000-0000-4000-8000-000000000001","name":"A\u0000B\n{long}","url":"http://a.example.org/x","tags":"{tags}","state":"{long}","favicon":"javascript:alert(1)"}}"#
        );
        let station = raw(&json).into_station().unwrap();
        assert!(station.name.chars().count() <= 80 && !station.name.contains('\u{0}'));
        assert_eq!(station.tags.len(), 8);
        assert_eq!(station.region.unwrap().chars().count(), MAX_TEXT_CHARS);
        assert_eq!(station.favicon, None);
    }

    #[test]
    fn search_sends_the_right_params() {
        let server = FakeDirectory::start(vec![FakeStation::new(1)]);
        let query = StationQuery {
            name: Some(" demo ".into()),
            country_code: Some("de".into()),
            region: Some("Testland".into()),
            tag: Some("pop".into()),
            geo: Some((50.0, 10.0, 75_000.0)),
            order: Order::Votes,
            offset: 20,
            limit: 30,
        };
        directory(&[&server]).search(&query).unwrap();
        let requests = server.requests();
        let sent = &requests[0];
        assert_eq!(sent.path, "/json/stations/search");
        for (key, value) in [
            ("hidebroken", "true"),
            ("name", "demo"),
            ("countrycode", "DE"),
            ("state", "Testland"),
            ("stateExact", "true"),
            ("tag", "pop"),
            ("has_geo_info", "true"),
            ("geo_lat", "50"),
            ("geo_long", "10"),
            ("geo_distance", "75000"),
            ("order", "votes"),
            ("reverse", "true"),
            ("offset", "20"),
            ("limit", "30"),
        ] {
            assert_eq!(sent.param(key), Some(value), "{key}");
        }
        directory(&[&server])
            .search(&StationQuery::default())
            .unwrap();
        let plain = &server.requests()[1];
        assert_eq!(plain.param("hidebroken"), Some("true"));
        assert_eq!(plain.param("geo_lat"), None);
        assert_eq!(plain.param("has_geo_info"), None);
    }

    #[test]
    fn limit_is_capped() {
        let server = FakeDirectory::start(vec![]);
        let query = StationQuery {
            limit: 10_000,
            ..StationQuery::default()
        };
        directory(&[&server]).search(&query).unwrap();
        assert_eq!(server.requests()[0].param("limit"), Some("500"));
    }

    #[test]
    fn user_agent_is_sent() {
        let server = FakeDirectory::start(vec![]);
        directory(&[&server]).countries().unwrap();
        assert_eq!(server.requests()[0].user_agent.as_deref(), Some(USER_AGENT));
        assert!(USER_AGENT.starts_with("Prism/"));
    }

    #[test]
    fn search_finds_and_filters() {
        let server = FakeDirectory::start(vec![
            FakeStation::new(1).clicks(5).geo(50.0, 10.0),
            FakeStation::new(2).clicks(50).geo(50.1, 10.1),
            FakeStation::new(3).clicks(500).geo(55.0, 20.0),
            FakeStation::new(4).clicks(9).broken(),
            FakeStation::new(5).codec("AAC,H.264", 0),
        ]);
        let dir = directory(&[&server]);
        let all = dir.search(&StationQuery::default()).unwrap();
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Demo Radio 3", "Demo Radio 2", "Demo Radio 1"]);
        let near = dir
            .search(&StationQuery {
                geo: Some((50.0, 10.0, 75_000.0)),
                ..StationQuery::default()
            })
            .unwrap();
        let names: Vec<&str> = near.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Demo Radio 2", "Demo Radio 1"]);
        assert!(near[0].distance_km.unwrap() > 10.0);
    }

    #[test]
    fn station_builders_reach_the_wire() {
        let station = FakeStation::new(7)
            .name("Demo Radio Seven")
            .url("http://127.0.0.1:9/live")
            .country("at");
        let id = station.id();
        let server = FakeDirectory::start(vec![station, FakeStation::new(8)]);
        let dir = directory(&[&server]);
        let found = dir
            .search(&StationQuery {
                country_code: Some("AT".into()),
                ..StationQuery::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, id);
        assert_eq!(found[0].name, "Demo Radio Seven");
        assert_eq!(found[0].url, "http://127.0.0.1:9/live");
        assert_eq!(found[0].country_code.as_deref(), Some("AT"));
    }

    #[test]
    fn rotates_to_the_next_mirror() {
        let down = FakeDirectory::start(vec![]);
        down.set_fault(Fault::Refuse);
        let up = FakeDirectory::start(vec![FakeStation::new(1)]);
        let dir = directory(&[&down, &up]);
        assert_eq!(dir.search(&StationQuery::default()).unwrap().len(), 1);
        assert_eq!((down.requests().len(), up.requests().len()), (1, 1));
        dir.search(&StationQuery::default()).unwrap();
        assert_eq!((down.requests().len(), up.requests().len()), (1, 2));
    }

    #[test]
    fn server_errors_and_stalls_rotate() {
        let broken = FakeDirectory::start(vec![]);
        broken.set_fault(Fault::Status(503));
        let slow = FakeDirectory::start(vec![]);
        slow.set_fault(Fault::Stall(Duration::from_millis(1500)));
        let up = FakeDirectory::start(vec![FakeStation::new(1)]);
        let dir = Directory::with_timeouts(
            MirrorSource::Fixed(vec![broken.url(), slow.url(), up.url()]),
            Duration::from_secs(1),
            Duration::from_millis(400),
        )
        .unwrap();
        assert_eq!(dir.search(&StationQuery::default()).unwrap().len(), 1);
        assert_eq!(up.requests().len(), 1);
    }

    #[test]
    fn a_restarted_mirror_is_found_again() {
        let mut only = FakeDirectory::start(vec![FakeStation::new(1)]);
        let dir = directory(&[&only]);
        only.stop();
        assert!(dir
            .search(&StationQuery::default())
            .unwrap_err()
            .is_offline());
        only.restart();
        assert_eq!(dir.search(&StationQuery::default()).unwrap().len(), 1);
    }

    #[test]
    fn client_errors_do_not_rotate() {
        let refusing = FakeDirectory::start(vec![]);
        refusing.set_fault(Fault::Status(404));
        let up = FakeDirectory::start(vec![FakeStation::new(1)]);
        let dir = directory(&[&refusing, &up]);
        let err = dir.search(&StationQuery::default()).unwrap_err();
        assert_eq!(err, DirectoryError::Server(404));
        assert_eq!(err.code(), "server");
        assert!(!err.is_offline() && !err.is_down());
        assert_eq!((refusing.requests().len(), up.requests().len()), (1, 0));
    }

    #[test]
    fn all_mirrors_down_is_unreachable() {
        let mut a = FakeDirectory::start(vec![]);
        let mut b = FakeDirectory::start(vec![]);
        let dir = directory(&[&a, &b]);
        a.stop();
        b.stop();
        let err = dir.search(&StationQuery::default()).unwrap_err();
        assert_eq!(err.code(), "unreachable");
        assert!(err.is_offline() && err.is_down());
    }

    #[test]
    fn huge_body_is_refused() {
        let server = FakeDirectory::start(vec![]);
        let dir = directory(&[&server]);
        // Sized: refused by Content-Length. Unsized: refused by the read cap.
        for fault in [Fault::Huge, Fault::HugeUnsized] {
            server.set_fault(fault);
            let err = dir.search(&StationQuery::default()).unwrap_err();
            assert_eq!(err.code(), "badResponse", "{fault:?}");
        }
    }

    #[test]
    fn a_non_list_names_the_host() {
        let server = FakeDirectory::start(vec![]);
        server.set_fault(Fault::Html);
        let text = directory(&[&server]).top_tags(5).unwrap_err().to_string();
        assert_eq!(
            text,
            "the directory sent something other than a list (127.0.0.1)"
        );
    }

    #[test]
    fn html_instead_of_json_is_a_bad_response() {
        let server = FakeDirectory::start(vec![]);
        server.set_fault(Fault::Html);
        let err = directory(&[&server]).top_tags(10).unwrap_err();
        assert_eq!(err.code(), "badResponse");
    }

    #[test]
    fn by_uuids_batches_by_50() {
        let stations: Vec<FakeStation> = (1..=120).map(FakeStation::new).collect();
        let uuids: Vec<String> = stations.iter().map(FakeStation::uuid).collect();
        let server = FakeDirectory::start(stations);
        let mut wanted: Vec<&str> = uuids.iter().map(String::as_str).collect();
        wanted.push("not-a-uuid");
        wanted.push(&uuids[0]);
        let found = directory(&[&server]).by_uuids(&wanted).unwrap();
        assert_eq!(found.len(), 120);
        let sizes: Vec<usize> = server
            .requests()
            .iter()
            .map(|r| r.param("uuids").unwrap().split(',').count())
            .collect();
        assert_eq!(sizes, [50, 50, 20]);
        assert!(directory(&[&server]).by_uuids(&[]).unwrap().is_empty());
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn click_hits_the_url_endpoint() {
        let server = FakeDirectory::start(vec![]);
        let dir = directory(&[&server]);
        dir.click("00000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(
            server.requests()[0].path,
            "/json/url/00000000-0000-4000-8000-000000000001"
        );
        assert!(dir.click("../etc/passwd").is_err());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn lists_parse() {
        let server = FakeDirectory::start(vec![]);
        let dir = directory(&[&server]);
        let countries = dir.countries().unwrap();
        assert_eq!(countries.len(), 3);
        assert_eq!(
            countries[0],
            Country {
                code: "DE".into(),
                name: "Germany".into(),
                station_count: 5200
            }
        );
        assert_eq!(countries[2].station_count, 42);
        let states = dir.states("Germany").unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(server.requests()[1].param("country"), Some("Germany"));
        let tags = dir.top_tags(10_000).unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[1].name, "rock");
        assert_eq!(tags[2].station_count, 880);
        assert_eq!(server.requests()[2].param("limit"), Some("500"));
    }

    #[test]
    fn mirror_discovery_dedupes_and_validates() {
        let server = FakeDirectory::start(vec![]);
        let mut dir = Directory::new(MirrorSource::Discover { cached: vec![] }).unwrap();
        assert_eq!(dir.mirrors(), [FALLBACK_MIRROR]);
        dir.set_discovery_base(&server.url());
        let mut names = dir.refresh_mirrors().unwrap();
        names.sort();
        assert_eq!(
            names,
            ["de1.api.radio-browser.info", "fr1.api.radio-browser.info"]
        );
        assert_eq!(dir.mirrors().len(), 2);
        let cached = Directory::new(MirrorSource::Discover {
            cached: vec![
                "FR1.api.radio-browser.info".into(),
                "bad host".into(),
                "x/../y".into(),
            ],
        })
        .unwrap();
        assert_eq!(cached.mirrors(), ["fr1.api.radio-browser.info"]);
        assert!(Directory::new(MirrorSource::Fixed(vec!["ftp://x".into()])).is_err());
    }

    #[test]
    fn errors_name_the_host_only() {
        let mut gone = FakeDirectory::start(vec![]);
        let dir = directory(&[&gone]);
        gone.stop();
        let query = StationQuery {
            name: Some("secret-search".into()),
            ..StationQuery::default()
        };
        let text = dir.search(&query).unwrap_err().to_string();
        assert!(text.contains("127.0.0.1"), "{text}");
        for leak in ["secret-search", "/json", "hidebroken", "http://", ":"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
    }
}
