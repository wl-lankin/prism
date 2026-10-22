//! The station model: ids, name and tag cleanup, and a station as a player track.

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::library::model::{Source, Track};

pub const RADIO_PREFIX: &str = "radio:";
pub const RADIO_COVER_PREFIX: &str = "rb-";

const MAX_URL_BYTES: usize = 2048;
const MAX_NAME_CHARS: usize = 80;
const MAX_TAGS: usize = 8;
const MAX_TAG_CHARS: usize = 40;
/// Trimmed from both ends of a station name: directories are full of banner-style names.
const NAME_DECORATION: &[char] = &['-', '#', '*', '|', '>', '~', '_', '.', ' '];

/// A radio station as the directory describes it, cleaned. `id` is `radio:<stationuuid>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: String,
    pub name: String,
    pub url: String,
    pub url_resolved: Option<String>,
    pub homepage: Option<String>,
    pub favicon: Option<String>,
    pub cover_id: Option<String>,
    pub country_code: Option<String>,
    pub region: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub hls: bool,
    pub last_check_ok: bool,
    pub distance_km: Option<f32>,
}

impl Station {
    /// The station as the queue and the engine see it. Duration 0 together with
    /// `Source::Radio` is what the engine calls live.
    pub fn as_track(&self) -> Track {
        let cover_id = station_cover_id(self.favicon.as_deref(), self.homepage.as_deref());
        Track {
            id: self.id.clone(),
            source: Source::Radio,
            title: self.name.clone(),
            artist: self.place_line(),
            album_artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            track_no: None,
            disc_no: None,
            duration_ms: 0,
            year: None,
            genre: None,
            format: self.codec.clone().unwrap_or_default(),
            sample_rate: None,
            bit_depth: None,
            bitrate_kbps: self.bitrate_kbps.filter(|rate| *rate > 0),
            cover_id,
            path: None,
            replay_gain_track_db: None,
            replay_gain_album_db: None,
            replay_gain_track_peak: None,
            replay_gain_album_peak: None,
        }
    }

    /// "<region>, <country name>", or whichever of the two is known.
    fn place_line(&self) -> String {
        let country = self.country_code.as_deref().map(country_name);
        let region = self
            .region
            .as_deref()
            .filter(|region| !region.is_empty())
            .map(str::to_string);
        match (region, country) {
            (Some(region), Some(country)) => format!("{region}, {country}"),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => String::new(),
        }
    }
}

fn is_uuid_shape(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// `radio:<uuid>` for a UUID in either case, `None` for anything else.
pub fn station_id(uuid: &str) -> Option<String> {
    is_uuid_shape(uuid).then(|| format!("{RADIO_PREFIX}{}", uuid.to_ascii_lowercase()))
}

/// The UUID of a station id as `station_id` builds it.
pub fn station_uuid(id: &str) -> Option<&str> {
    let uuid = id.strip_prefix(RADIO_PREFIX)?;
    (is_uuid_shape(uuid) && !uuid.bytes().any(|byte| byte.is_ascii_uppercase())).then_some(uuid)
}

/// An http(s) URL without whitespace or control characters, of at most 2 048 bytes.
pub(crate) fn http_url(url: &str) -> Option<&str> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let schemed = lower.starts_with("http://") || lower.starts_with("https://");
    let clean = !url.chars().any(|ch| ch.is_control() || ch.is_whitespace());
    (schemed && clean && url.len() <= MAX_URL_BYTES).then_some(url)
}

fn hashed_cover_id(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("{RADIO_COVER_PREFIX}{}", &digest[..20])
}

/// `rb-<first 20 hex of sha1(url)>` for an http(s) URL of at most 2 048 bytes.
pub fn favicon_cover_id(url: &str) -> Option<String> {
    http_url(url).map(hashed_cover_id)
}

/// The 20 lowercase hex digits of a logo cover id, `None` for anything else.
pub fn radio_cover_raw(id: &str) -> Option<&str> {
    let raw = id.strip_prefix(RADIO_COVER_PREFIX)?;
    let hex = raw.len() == 20
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    hex.then_some(raw)
}

/// The cover id of a homepage logo: `rb-<first 20 hex of sha1("home:" + homepage)>`.
pub fn homepage_cover_id(homepage: &str) -> Option<String> {
    http_url(homepage).map(|url| hashed_cover_id(&format!("home:{url}")))
}

/// The id of a station's logo: its favicon's, else its homepage's.
pub fn station_cover_id(favicon: Option<&str>, homepage: Option<&str>) -> Option<String> {
    favicon
        .and_then(favicon_cover_id)
        .or_else(|| homepage.and_then(homepage_cover_id))
}

/// Control characters removed, whitespace collapsed, decoration trimmed, at most 80
/// characters (cut on a char boundary, ending in `...`). `None` when nothing is left.
pub fn clean_name(raw: &str) -> Option<String> {
    let spaced: String = raw
        .chars()
        .filter_map(|ch| {
            if ch.is_whitespace() {
                Some(' ')
            } else if ch.is_control() || is_format_char(ch) {
                None
            } else {
                Some(ch)
            }
        })
        .collect();
    let collapsed = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(NAME_DECORATION);
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MAX_NAME_CHARS {
        return Some(trimmed.to_string());
    }
    let head: String = trimmed.chars().take(MAX_NAME_CHARS - 3).collect();
    let mut cut = head.trim_end_matches(NAME_DECORATION).to_string();
    cut.push_str("...");
    Some(cut)
}

/// Invisible Unicode format characters (zero-width, bidi controls, BOM) that would spoof or
/// hide parts of a name.
pub(crate) fn is_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Lowercased, split on commas, trimmed, deduplicated, at most 8 kept.
pub fn clean_tags(raw: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let tag: String = part
            .chars()
            .filter(|ch| !ch.is_control() && !is_format_char(*ch))
            .collect::<String>()
            .trim()
            .chars()
            .take(MAX_TAG_CHARS)
            .collect::<String>()
            .trim()
            .to_lowercase();
        if tag.is_empty() || tags.contains(&tag) {
            continue;
        }
        tags.push(tag);
        if tags.len() == MAX_TAGS {
            break;
        }
    }
    tags
}

/// The 60 most common station countries; anything else shows its code.
const COUNTRIES: &[(&str, &str)] = &[
    ("AR", "Argentina"),
    ("AT", "Austria"),
    ("AU", "Australia"),
    ("BE", "Belgium"),
    ("BG", "Bulgaria"),
    ("BR", "Brazil"),
    ("CA", "Canada"),
    ("CH", "Switzerland"),
    ("CL", "Chile"),
    ("CN", "China"),
    ("CO", "Colombia"),
    ("CZ", "Czechia"),
    ("DE", "Germany"),
    ("DK", "Denmark"),
    ("DZ", "Algeria"),
    ("EC", "Ecuador"),
    ("EE", "Estonia"),
    ("EG", "Egypt"),
    ("ES", "Spain"),
    ("FI", "Finland"),
    ("FR", "France"),
    ("GB", "United Kingdom"),
    ("GR", "Greece"),
    ("HR", "Croatia"),
    ("HU", "Hungary"),
    ("ID", "Indonesia"),
    ("IE", "Ireland"),
    ("IL", "Israel"),
    ("IN", "India"),
    ("IR", "Iran"),
    ("IS", "Iceland"),
    ("IT", "Italy"),
    ("JP", "Japan"),
    ("KR", "South Korea"),
    ("LT", "Lithuania"),
    ("LU", "Luxembourg"),
    ("LV", "Latvia"),
    ("MA", "Morocco"),
    ("MX", "Mexico"),
    ("MY", "Malaysia"),
    ("NG", "Nigeria"),
    ("NL", "Netherlands"),
    ("NO", "Norway"),
    ("NZ", "New Zealand"),
    ("PE", "Peru"),
    ("PH", "Philippines"),
    ("PL", "Poland"),
    ("PT", "Portugal"),
    ("RO", "Romania"),
    ("RS", "Serbia"),
    ("RU", "Russia"),
    ("SE", "Sweden"),
    ("SI", "Slovenia"),
    ("SK", "Slovakia"),
    ("TH", "Thailand"),
    ("TR", "Turkey"),
    ("UA", "Ukraine"),
    ("US", "United States"),
    ("VE", "Venezuela"),
    ("ZA", "South Africa"),
];

fn country_name(code: &str) -> String {
    let upper = code.to_ascii_uppercase();
    COUNTRIES
        .iter()
        .find(|(known, _)| *known == upper)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or(upper)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const UUID: &str = "9617a958-0601-11e8-ae97-52543be04c81";

    /// A station with fake data; `n` varies the uuid (as a hex digit run) and the name.
    pub(crate) fn station(n: u32) -> Station {
        Station {
            id: format!("{RADIO_PREFIX}00000000-0000-4000-8000-{n:012x}"),
            name: format!("Demo Radio {n}"),
            url: format!("http://127.0.0.1:1/stream{n}"),
            url_resolved: None,
            homepage: None,
            favicon: None,
            cover_id: None,
            country_code: Some("DE".to_string()),
            region: Some("Testland".to_string()),
            language: None,
            tags: vec!["pop".to_string()],
            codec: Some("MP3".to_string()),
            bitrate_kbps: Some(128),
            hls: false,
            last_check_ok: true,
            distance_km: None,
        }
    }

    #[test]
    fn station_ids_validate() {
        assert_eq!(
            station_id("9617A958-0601-11E8-AE97-52543BE04C81").as_deref(),
            Some("radio:9617a958-0601-11e8-ae97-52543be04c81")
        );
        assert_eq!(
            station_id(UUID).as_deref(),
            Some("radio:9617a958-0601-11e8-ae97-52543be04c81")
        );
        assert_eq!(station_id("../x"), None);
        assert_eq!(station_id(""), None);
        assert_eq!(station_id(&format!("{UUID}a")), None);
        assert_eq!(station_id("9617a958_0601-11e8-ae97-52543be04c81"), None);
        assert_eq!(
            station_uuid("radio:9617a958-0601-11e8-ae97-52543be04c81"),
            Some(UUID)
        );
        assert_eq!(station_uuid("local:abc"), None);
        assert_eq!(station_uuid("radio:../x"), None);
        assert_eq!(
            station_uuid("radio:9617A958-0601-11E8-AE97-52543BE04C81"),
            None
        );
    }

    #[test]
    fn favicon_cover_id_is_stable_and_safe() {
        let id = favicon_cover_id("https://example.test/logo.png").unwrap();
        assert_eq!(
            id,
            favicon_cover_id("https://example.test/logo.png").unwrap()
        );
        assert_ne!(
            id,
            favicon_cover_id("https://example.test/other.png").unwrap()
        );
        assert!(id.starts_with("rb-") && id.len() == 3 + 20);
        assert!(id[3..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(favicon_cover_id("ftp://example.test/logo.png"), None);
        assert_eq!(favicon_cover_id("javascript:alert(1)"), None);
        assert_eq!(favicon_cover_id(""), None);
        let long = format!("https://example.test/{}", "a".repeat(3000));
        assert_eq!(favicon_cover_id(&long), None);
        assert_eq!(favicon_cover_id("https://example.test/a b.png"), None);
        assert_eq!(radio_cover_raw(&id), Some(&id[3..]));
        for bad in [
            "rb-",
            "rb-../x",
            "nd-0123456789abcdef0123",
            "rb-0123456789ABCDEF0123",
        ] {
            assert_eq!(radio_cover_raw(bad), None, "{bad}");
        }
    }

    #[test]
    fn station_cover_id_prefers_the_favicon_then_the_homepage() {
        let icon = "https://example.test/logo.png";
        let home = "https://example.test/";
        assert_eq!(
            station_cover_id(Some(icon), Some(home)),
            favicon_cover_id(icon)
        );
        let from_home = station_cover_id(None, Some(home)).unwrap();
        assert_ne!(Some(from_home.clone()), favicon_cover_id(home));
        assert_eq!(Some(from_home.clone()), homepage_cover_id(home));
        // A favicon that is no usable URL falls back to the homepage.
        assert_eq!(
            station_cover_id(Some("ftp://x/y.png"), Some(home)),
            Some(from_home)
        );
        assert_eq!(station_cover_id(None, None), None);
        assert_eq!(station_cover_id(Some(""), Some("mailto:a@b.test")), None);
    }

    #[test]
    fn clean_name_cases() {
        assert_eq!(
            clean_name(" - DJ & CLUB CHARTS ---> Club").as_deref(),
            Some("DJ & CLUB CHARTS ---> Club")
        );
        assert_eq!(
            clean_name("*CLUB CHARTS TOP 100 || I").as_deref(),
            Some("CLUB CHARTS TOP 100 || I")
        );
        assert_eq!(
            clean_name("\u{0007}Radio\n  Eins").as_deref(),
            Some("Radio Eins")
        );
        assert_eq!(clean_name("###"), None);
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(""), None);
        let long = clean_name(&"x".repeat(2000)).unwrap();
        assert_eq!(long.chars().count(), 80);
        assert!(long.ends_with("..."));
        let wide = clean_name(&"ä".repeat(500)).unwrap();
        assert_eq!(wide.chars().count(), 80);
        assert!(wide.ends_with("..."));
        let emoji = clean_name(&"😀".repeat(100)).unwrap();
        assert_eq!(emoji.chars().count(), 80);
        assert_eq!(clean_name(&"y".repeat(80)).unwrap().chars().count(), 80);
    }

    #[test]
    fn clean_name_cut_end_is_trimmed_before_the_ellipsis() {
        // The 77th char is a space and a dash: they must not sit in front of "...".
        let name = format!("{}- {}", "a".repeat(75), "b".repeat(50));
        let cut = clean_name(&name).unwrap();
        assert_eq!(cut, format!("{}...", "a".repeat(75)));
        assert!(cut.chars().count() <= 80);
        let plain = clean_name(&format!("{} {}", "a".repeat(76), "b".repeat(50))).unwrap();
        assert_eq!(plain, format!("{}...", "a".repeat(76)));
    }

    #[test]
    fn invisible_format_characters_are_dropped() {
        assert_eq!(
            clean_name("Ra\u{200B}dio\u{202E} Ei\u{2066}ns\u{FEFF}").as_deref(),
            Some("Radio Eins")
        );
        assert_eq!(clean_name("\u{200B}\u{2060}\u{FEFF}"), None);
        assert_eq!(
            clean_tags("po\u{200D}p, \u{202E}rock\u{2069}"),
            vec!["pop", "rock"]
        );
        // A zero-width character does not make two tags different.
        assert_eq!(clean_tags("pop,p\u{200B}op"), vec!["pop"]);
    }

    #[test]
    fn clean_tags_trims_before_the_cut() {
        let padded = format!("   {}", "t".repeat(60));
        assert_eq!(clean_tags(&padded)[0], "t".repeat(40));
        let spaced = format!("{}  ", "t".repeat(39));
        assert_eq!(clean_tags(&spaced)[0], "t".repeat(39));
    }

    #[test]
    fn clean_tags_dedupes_and_caps() {
        assert_eq!(
            clean_tags(" Pop, ROCK ,pop,, \u{0007}jazz"),
            vec!["pop", "rock", "jazz"]
        );
        let many = (0..300)
            .map(|n| format!("tag{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let tags = clean_tags(&many);
        assert_eq!(tags.len(), 8);
        assert_eq!(tags[0], "tag0");
        assert!(clean_tags("").is_empty());
        assert_eq!(clean_tags(&"t".repeat(500))[0].len(), 40);
    }

    #[test]
    fn station_as_track_is_live() {
        let mut station = station(1);
        station.favicon = Some("https://example.test/logo.png".to_string());
        let track = station.as_track();
        assert_eq!(track.duration_ms, 0);
        assert_eq!(track.source, Source::Radio);
        assert_eq!(track.path, None);
        assert_eq!(track.id, station.id);
        assert_eq!(track.title, "Demo Radio 1");
        assert_eq!(track.artist, "Testland, Germany");
        assert_eq!(track.format, "MP3");
        assert_eq!(track.bitrate_kbps, Some(128));
        assert_eq!(
            track.cover_id,
            favicon_cover_id("https://example.test/logo.png")
        );
        assert_eq!(track.album, "");
        assert_eq!(track.replay_gain_track_db, None);
    }

    #[test]
    fn place_line_falls_back_to_the_code_or_the_region() {
        let mut station = station(2);
        station.country_code = Some("zz".to_string());
        assert_eq!(station.as_track().artist, "Testland, ZZ");
        station.region = None;
        assert_eq!(station.as_track().artist, "ZZ");
        station.country_code = None;
        assert_eq!(station.as_track().artist, "");
        station.bitrate_kbps = Some(0);
        assert_eq!(station.as_track().bitrate_kbps, None);
    }

    #[test]
    fn country_table_is_sorted_and_unique() {
        let codes: Vec<&str> = COUNTRIES.iter().map(|(code, _)| *code).collect();
        assert_eq!(codes.len(), 60);
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, codes);
    }
}
