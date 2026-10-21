//! Station logos for the resize gate (`rb-` covers): the station's favicon, else a logo its
//! homepage names (apple-touch-icon, icon, og:image, `/favicon.ico`).
//!
//! Every fetch is capped (size, time, redirects) and decoded with bounds before the gate sees
//! the bytes. A logo that no candidate delivers is remembered as missing for 7 days, so a
//! scrolling list does not ask again; a host that does not answer at all is not remembered
//! (the user may just be offline). Logo URLs come from directory data, so no fetch, redirect or
//! resolved host name may reach a private network; such a URL counts as missing.

use std::future::Future;
use std::io::{Cursor, Read};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use reqwest::Url;

use super::directory::USER_AGENT;
use super::model::RADIO_COVER_PREFIX;
use super::store;
use crate::db::Db;
use crate::library::scan::lock;
use crate::thumbs::{RemoteCovers, ResizeError, ResizeGate};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const FAVICON_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_FAVICON: u64 = 1024 * 1024;
pub const HOMEPAGE_TIMEOUT: Duration = Duration::from_secs(8);
pub const MAX_HOMEPAGE: u64 = 512 * 1024;
pub const MAX_REDIRECTS: usize = 3;
/// At most this many logo fetches run at once.
pub const FAVICON_FETCH_LIMIT: usize = 4;
pub const MISSING_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const MAX_CANDIDATES: usize = 16;
/// One logo never takes longer than this, however many candidates a homepage names.
const COVER_DEADLINE: Duration = Duration::from_secs(30);
const MAX_DIMENSION: u32 = 4096;
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// What one request gave.
enum Attempt<T> {
    Got(T),
    /// The host answered, but not with something usable.
    Answered,
    /// No answer at all: refused, timed out, DNS.
    NoAnswer,
}

/// A logo URL that points into a private network (the user's router, the machine itself):
/// directory data must never make Prism ask there.
#[derive(Debug)]
struct PrivateAddress;

impl std::fmt::Display for PrivateAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a logo address in a private network")
    }
}

impl std::error::Error for PrivateAddress {}

/// True for an address on the public internet: not loopback, private, link local, shared
/// (100.64/10), unspecified, multicast, broadcast or unique local; IPv4-mapped IPv6 counts as
/// its IPv4 address.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public(IpAddr::V4(v4)),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (first & 0xffc0) == 0xfe80
                    || (first & 0xfe00) == 0xfc00)
            }
        },
    }
}

/// False for a URL whose host is a literal non-public address. Host names are checked by
/// [`PublicResolver`] after the lookup.
fn url_allowed(url: &Url, allow_private: bool) -> bool {
    let literal = url
        .host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|host| host.parse::<IpAddr>().ok());
    allow_private || literal.is_none_or(is_public)
}

fn refused_as_private(err: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(cause) = source {
        if cause.is::<PrivateAddress>() {
            return true;
        }
        source = cause.source();
    }
    false
}

type Lookup = Result<Addrs, Box<dyn std::error::Error + Send + Sync>>;

/// Resolves with the system resolver and keeps only public addresses; none left is an error.
/// The system lookup blocks, so it runs on a thread of its own.
struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(BlockingLookup::start(name.as_str().to_owned()))
    }
}

struct BlockingLookup {
    slot: Arc<Mutex<(Option<Lookup>, Option<Waker>)>>,
}

impl BlockingLookup {
    fn start(host: String) -> BlockingLookup {
        let slot: Arc<Mutex<(Option<Lookup>, Option<Waker>)>> = Arc::default();
        let filled = Arc::clone(&slot);
        let finish = move |result: Lookup| {
            let mut slot = filled.lock().unwrap_or_else(|e| e.into_inner());
            slot.0 = Some(result);
            if let Some(waker) = slot.1.take() {
                waker.wake();
            }
        };
        let spawned = std::thread::Builder::new()
            .name("prism-logo-dns".into())
            .spawn({
                let finish = finish.clone();
                move || finish(public_addrs(&host))
            });
        if let Err(err) = spawned {
            finish(Err(Box::new(err)));
        }
        BlockingLookup { slot }
    }
}

impl Future for BlockingLookup {
    type Output = Lookup;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Lookup> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match slot.0.take() {
            Some(result) => Poll::Ready(result),
            None => {
                slot.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

fn public_addrs(host: &str) -> Lookup {
    let all: Vec<SocketAddr> = (host, 0).to_socket_addrs()?.collect();
    if all.is_empty() {
        return Err(Box::new(std::io::Error::from(std::io::ErrorKind::NotFound)));
    }
    let public: Vec<SocketAddr> = all.into_iter().filter(|a| is_public(a.ip())).collect();
    if public.is_empty() {
        return Err(Box::new(PrivateAddress));
    }
    Ok(Box::new(public.into_iter()))
}

pub struct FaviconCovers {
    db: Arc<Db>,
    client: Client,
    /// Only the tests, whose hosts are on 127.0.0.1, allow private addresses.
    allow_private: bool,
}

impl FaviconCovers {
    pub fn new(db: Arc<Db>) -> anyhow::Result<FaviconCovers> {
        FaviconCovers::with_private(db, false)
    }

    pub(crate) fn with_private(db: Arc<Db>, allow_private: bool) -> anyhow::Result<FaviconCovers> {
        let mut builder = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::custom(move |attempt| {
                // `previous` holds every URL asked so far, the first one included.
                if attempt.previous().len() > MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if !matches!(attempt.url().scheme(), "http" | "https") {
                    attempt.stop()
                } else if !url_allowed(attempt.url(), allow_private) {
                    attempt.error(PrivateAddress)
                } else {
                    attempt.follow()
                }
            }));
        if !allow_private {
            builder = builder.dns_resolver(Arc::new(PublicResolver));
        }
        Ok(FaviconCovers {
            db,
            client: builder.build()?,
            allow_private,
        })
    }

    fn get(&self, url: &Url, timeout: Duration, accept: &str) -> Attempt<Response> {
        #[cfg(test)]
        crate::radio::testserver::assert_loopback(url.as_str());
        // Refused like a host that answered with nothing: remembered as missing.
        if !url_allowed(url, self.allow_private) {
            return Attempt::Answered;
        }
        match self
            .client
            .get(url.clone())
            .timeout(timeout)
            .header(reqwest::header::ACCEPT, accept)
            .send()
        {
            Ok(response) if response.status().is_success() => Attempt::Got(response),
            Ok(_) => Attempt::Answered,
            Err(err) if err.is_redirect() || err.is_status() || refused_as_private(&err) => {
                Attempt::Answered
            }
            Err(_) => Attempt::NoAnswer,
        }
    }

    /// An image of at most [`MAX_FAVICON`] that decodes within bounds.
    fn image(&self, url: &Url) -> Attempt<Vec<u8>> {
        let response = match self.get(url, FAVICON_TIMEOUT, "image/*") {
            Attempt::Got(response) => response,
            Attempt::Answered => return Attempt::Answered,
            Attempt::NoAnswer => return Attempt::NoAnswer,
        };
        if response
            .content_length()
            .is_some_and(|len| len > MAX_FAVICON)
        {
            return Attempt::Answered;
        }
        let mut bytes = Vec::new();
        if response
            .take(MAX_FAVICON + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Attempt::NoAnswer;
        }
        if bytes.len() as u64 > MAX_FAVICON || !decodes(&bytes) {
            return Attempt::Answered;
        }
        Attempt::Got(bytes)
    }

    /// The page after redirects and its first [`MAX_HOMEPAGE`] bytes as text; `None` when the
    /// address answers with something other than a page.
    fn homepage(&self, url: &Url) -> Attempt<Option<(Url, String)>> {
        let response = match self.get(url, HOMEPAGE_TIMEOUT, "text/html") {
            Attempt::Got(response) => response,
            Attempt::Answered => return Attempt::Answered,
            Attempt::NoAnswer => return Attempt::NoAnswer,
        };
        // Many directory "homepages" are the stream itself; its server's own favicon (Icecast's
        // note) is not the station's logo, so only a real page is asked for icons.
        let html = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                let value = value.to_ascii_lowercase();
                value.starts_with("text/html") || value.starts_with("application/xhtml")
            });
        if !html {
            return Attempt::Got(None);
        }
        let page = response.url().clone();
        let mut bytes = Vec::new();
        // A page cut short still names its icons in the head.
        let read = response.take(MAX_HOMEPAGE).read_to_end(&mut bytes);
        if read.is_err() && bytes.is_empty() {
            return Attempt::Answered;
        }
        Attempt::Got(Some((page, String::from_utf8_lossy(&bytes).into_owned())))
    }

    /// Tries `url` once per logo; `answered` notes that some host answered.
    fn try_image(&self, url: Url, tried: &mut Vec<Url>, answered: &mut bool) -> Option<Vec<u8>> {
        if tried.contains(&url) {
            return None;
        }
        let attempt = self.image(&url);
        tried.push(url);
        match attempt {
            Attempt::Got(bytes) => Some(bytes),
            Attempt::Answered => {
                *answered = true;
                None
            }
            Attempt::NoAnswer => None,
        }
    }
}

type Response = reqwest::blocking::Response;

fn internal(err: anyhow::Error) -> ResizeError {
    ResizeError::Internal(format!("{err:#}"))
}

fn http_url(raw: &str) -> Option<Url> {
    Url::parse(raw)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
}

/// True when the bytes are an image the gate can decode, within bounds (svg and html are not).
fn decodes(bytes: &[u8]) -> bool {
    let Ok(mut reader) = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format() else {
        return false;
    };
    if reader.format().is_none() {
        return false;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    reader.decode().is_ok()
}

impl RemoteCovers for FaviconCovers {
    fn fetch(&self, raw: &str, _size: u32) -> Result<Vec<u8>, ResizeError> {
        let cover_id = format!("{RADIO_COVER_PREFIX}{raw}");
        let (source, missing_until) = {
            let conn = lock(&self.db).map_err(internal)?;
            (
                store::favicon_source(&conn, &cover_id).map_err(internal)?,
                store::favicon_missing_until(&conn, &cover_id).map_err(internal)?,
            )
        };
        if missing_until.is_some_and(|until| until > now_ms()) {
            return Err(ResizeError::Unknown);
        }
        let Some(source) = source else {
            return Err(ResizeError::Unknown);
        };
        let deadline = Instant::now() + COVER_DEADLINE;
        let mut answered = false;
        let mut tried = Vec::new();
        if let Some(url) = source.url.as_deref().and_then(http_url) {
            if let Some(bytes) = self.try_image(url, &mut tried, &mut answered) {
                return Ok(bytes);
            }
        }
        if let Some(home) = source.homepage.as_deref().and_then(http_url) {
            let candidates = match self.homepage(&home) {
                Attempt::Got(Some((page, html))) => {
                    answered = true;
                    icon_candidates(&html, &page)
                }
                Attempt::Got(None) => {
                    answered = true;
                    Vec::new()
                }
                Attempt::Answered => {
                    answered = true;
                    icon_candidates("", &home)
                }
                Attempt::NoAnswer => icon_candidates("", &home),
            };
            for url in candidates {
                if Instant::now() >= deadline {
                    break;
                }
                if let Some(bytes) = self.try_image(url, &mut tried, &mut answered) {
                    return Ok(bytes);
                }
            }
        }
        if !answered {
            return Err(ResizeError::Unreachable);
        }
        let marked = lock(&self.db).and_then(|conn| {
            store::mark_favicon_missing(&conn, &cover_id, now_ms() + MISSING_FOR_MS)
        });
        if let Err(err) = marked {
            eprintln!("remembering a missing station logo failed: {err:#}");
        }
        Err(ResizeError::Unknown)
    }

    fn fetch_limit(&self) -> usize {
        FAVICON_FETCH_LIMIT
    }
}

/// Plugs the logos into the gate.
pub fn attach(gate: &ResizeGate, db: &Arc<Db>) -> anyhow::Result<()> {
    gate.set_remote_for(
        RADIO_COVER_PREFIX,
        Arc::new(FaviconCovers::new(Arc::clone(db))?),
    );
    Ok(())
}

/// The logos a homepage names, best first: `apple-touch-icon` (largest `sizes`), `icon` and
/// `shortcut icon` (largest `sizes`, png before ico, svg skipped), `og:image`, then
/// `/favicon.ico` of `base`'s origin. Only http(s) URLs, relative ones resolved against
/// `base`, at most [`MAX_CANDIDATES`]. A small bounded tag scanner: the first 512 KiB, up to
/// `<body>` or `</head>`, comments, scripts and styles skipped.
pub(crate) fn icon_candidates(html: &str, base: &Url) -> Vec<Url> {
    let text = floor_boundary(html, MAX_HOMEPAGE as usize);
    let bytes = text.as_bytes();
    let mut found = Found::default();
    let mut tags = 0;
    let mut pos = 0;
    while tags < MAX_TAGS {
        let Some(offset) = bytes[pos..].iter().position(|b| *b == b'<') else {
            break;
        };
        let start = pos + offset;
        let rest = &bytes[start + 1..];
        if rest.starts_with(b"!--") {
            match find_ci(bytes, start + 4, b"-->") {
                Some(end) => pos = end + 3,
                None => break,
            }
            continue;
        }
        if rest.first() == Some(&b'/') {
            if starts_with_ci(&rest[1..], b"head") {
                break;
            }
            pos = start + 1;
            continue;
        }
        let name_len = rest
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        let name = &rest[..name_len];
        let after = start + 1 + name_len;
        if name.eq_ignore_ascii_case(b"body") {
            break;
        }
        let closing: Option<&[u8]> = if name.eq_ignore_ascii_case(b"script") {
            Some(b"</script")
        } else if name.eq_ignore_ascii_case(b"style") {
            Some(b"</style")
        } else {
            None
        };
        if let Some(closing) = closing {
            match find_ci(bytes, after, closing) {
                Some(end) => pos = end + closing.len(),
                None => break,
            }
            continue;
        }
        let link = name.eq_ignore_ascii_case(b"link");
        if link || name.eq_ignore_ascii_case(b"meta") {
            let limit = (start + MAX_TAG).min(bytes.len());
            match parse_attrs(text, after, limit) {
                Some((attrs, end)) => {
                    tags += 1;
                    found.add(link, &attrs, base);
                    pos = end + 1;
                }
                // An unclosed tag at the end of the page.
                None if limit == bytes.len() => break,
                None => pos = limit,
            }
            continue;
        }
        pos = after;
    }
    found.into_candidates(base)
}

/// Tags scanned for icons at most; a page with more is not a station's homepage.
const MAX_TAGS: usize = 64;
/// One tag never spans more.
const MAX_TAG: usize = 4096;
const MAX_ATTRS: usize = 16;

#[derive(Default)]
struct Found {
    /// Size, URL.
    apple: Vec<(u32, Url)>,
    /// Size, kind (0 png, 1 other, 2 ico), URL.
    icons: Vec<(u32, u8, Url)>,
    og: Vec<Url>,
}

impl Found {
    fn add(&mut self, link: bool, attrs: &[(String, &str)], base: &Url) {
        let get = |key: &str| {
            attrs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.trim().replace("&amp;", "&"))
        };
        if !link {
            let property = get("property").or_else(|| get("name")).unwrap_or_default();
            if property.eq_ignore_ascii_case("og:image") {
                if let Some(url) = get("content").and_then(|href| resolve(base, &href)) {
                    self.og.push(url);
                }
            }
            return;
        }
        let rel = get("rel").unwrap_or_default().to_ascii_lowercase();
        let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
        let apple = tokens
            .iter()
            .any(|t| matches!(*t, "apple-touch-icon" | "apple-touch-icon-precomposed"));
        let icon = tokens.contains(&"icon");
        let Some(href) = get("href").filter(|href| !href.is_empty()) else {
            return;
        };
        let kind = get("type").unwrap_or_default().to_ascii_lowercase();
        let lower = href.to_ascii_lowercase();
        let path = lower.split(['?', '#']).next().unwrap_or("");
        if !(apple || icon) || kind.contains("svg") || path.ends_with(".svg") {
            return;
        }
        let Some(url) = resolve(base, &href) else {
            return;
        };
        let size = get("sizes").map(|sizes| largest_size(&sizes)).unwrap_or(0);
        if apple {
            self.apple.push((size, url));
        } else {
            let rank = if kind.contains("png") || path.ends_with(".png") {
                0
            } else if kind.contains("icon") || path.ends_with(".ico") {
                2
            } else {
                1
            };
            self.icons.push((size, rank, url));
        }
    }

    fn into_candidates(mut self, base: &Url) -> Vec<Url> {
        self.apple.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
        self.icons
            .sort_by_key(|(size, kind, _)| (std::cmp::Reverse(*size), *kind));
        let named = self
            .apple
            .into_iter()
            .map(|(_, url)| url)
            .chain(self.icons.into_iter().map(|(_, _, url)| url))
            .chain(self.og);
        let mut out: Vec<Url> = Vec::new();
        for url in named {
            // One place stays for the origin's favicon.
            if out.len() >= MAX_CANDIDATES - 1 {
                break;
            }
            if !out.contains(&url) {
                out.push(url);
            }
        }
        if let Some(favicon) = resolve(base, "/favicon.ico") {
            if !out.contains(&favicon) {
                out.push(favicon);
            }
        }
        out
    }
}

/// The largest width in a `sizes` value such as `16x16 32x32`; `any` and junk count 0.
fn largest_size(sizes: &str) -> u32 {
    sizes
        .split_ascii_whitespace()
        .filter_map(|size| {
            let (width, _) = size.split_once(['x', 'X'])?;
            width.parse::<u32>().ok()
        })
        .map(|width| width.min(10_000))
        .max()
        .unwrap_or(0)
}

fn resolve(base: &Url, href: &str) -> Option<Url> {
    base.join(href)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.as_str().len() <= 2048)
}

fn floor_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn starts_with_ci(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn find_ci(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map(|at| from + at)
}

/// The attributes of a tag from `from` (after its name) to its `>`, which must come before
/// `limit`. Names are lowercased; values are slices of `text`.
fn parse_attrs(text: &str, from: usize, limit: usize) -> Option<(Vec<(String, &str)>, usize)> {
    let bytes = text.as_bytes();
    let mut attrs = Vec::new();
    let mut i = from;
    loop {
        while i < limit && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= limit {
            return None;
        }
        if bytes[i] == b'>' {
            return Some((attrs, i));
        }
        let start = i;
        while i < limit
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'=' | b'>' | b'/')
        {
            i += 1;
        }
        if i == start {
            // A stray `=`.
            i += 1;
            continue;
        }
        let name = String::from_utf8_lossy(&bytes[start..i]).to_ascii_lowercase();
        while i < limit && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = "";
        if i < limit && bytes[i] == b'=' {
            i += 1;
            while i < limit && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= limit {
                return None;
            }
            let quote = bytes[i];
            if quote == b'"' || quote == b'\'' {
                let open = i + 1;
                let close = open + bytes[open..limit].iter().position(|b| *b == quote)?;
                value = text.get(open..close).unwrap_or("");
                i = close + 1;
            } else {
                let open = i;
                while i < limit && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                if i >= limit {
                    return None;
                }
                value = text.get(open..i).unwrap_or("");
            }
        }
        if attrs.len() < MAX_ATTRS {
            attrs.push((name, value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::model::station_cover_id;
    use crate::radio::model::tests::station;
    use crate::radio::model::Station;
    use crate::radio::testserver::{FakeWeb, Page};
    use crate::thumbs::SIZES;
    use image::{Rgba, RgbaImage};

    fn base() -> Url {
        Url::parse("https://radio.example.org/station/home.html").unwrap()
    }

    fn strings(urls: &[Url]) -> Vec<String> {
        urls.iter().map(Url::to_string).collect()
    }

    #[test]
    fn icon_candidates_order() {
        let html = r#"<!doctype html><html><head>
            <meta property="og:image" content="https://cdn.example.org/og.jpg">
            <link rel="icon" type="image/png" sizes="32x32" href="/icon-32.png">
            <link rel="shortcut icon" href="favicon-legacy.ico">
            <link rel="icon" type="image/svg+xml" href="/logo.svg">
            <link rel="icon" href="/vector.SVG?v=2">
            <LINK REL="Apple-Touch-Icon" SIZES="180x180" HREF="/apple-180.png">
            <link rel="apple-touch-icon" sizes="57x57" href='/apple-57.png'>
            <link rel="icon" sizes="16x16" href="/icon-16.png">
            <link rel="icon" sizes="32x32" href="/icon-32.ico">
            <link rel="icon" href="javascript:alert(1)">
            <link rel="icon" href="data:image/png;base64,AAAA">
            <link rel="icon" href="ftp://files.example.org/x.png">
            <link rel=icon href=/unquoted.png sizes=8x8>
            <link rel="stylesheet" href="/style.css">
            <link rel="icon" href="/a.png?x=1&amp;y=2">
            </head><body><link rel="icon" href="/in-body.png"></body></html>"#;
        assert_eq!(
            strings(&icon_candidates(html, &base())),
            vec![
                "https://radio.example.org/apple-180.png",
                "https://radio.example.org/apple-57.png",
                "https://radio.example.org/icon-32.png",
                "https://radio.example.org/icon-32.ico",
                "https://radio.example.org/icon-16.png",
                "https://radio.example.org/unquoted.png",
                "https://radio.example.org/a.png?x=1&y=2",
                "https://radio.example.org/station/favicon-legacy.ico",
                "https://cdn.example.org/og.jpg",
                "https://radio.example.org/favicon.ico",
            ]
        );
        // Nothing named: the origin's favicon only.
        assert_eq!(
            strings(&icon_candidates("<p>no head</p>", &base())),
            vec!["https://radio.example.org/favicon.ico"]
        );
    }

    #[test]
    fn icon_candidates_skip_comments_and_scripts() {
        let html = r#"<head><!-- <link rel="icon" href="/commented.png"> -->
            <script>var s = '<link rel="icon" href="/scripted.png">';</script>
            <style>/* <link rel="icon" href="/styled.png"> */</style>
            <link rel="icon" href="/real.png"></head>"#;
        assert_eq!(
            strings(&icon_candidates(html, &base())),
            vec![
                "https://radio.example.org/real.png",
                "https://radio.example.org/favicon.ico",
            ]
        );
    }

    #[test]
    fn icon_scanner_is_bounded() {
        // A 5 MB page: only the first 512 KiB are read.
        let mut big = "<head>".to_owned();
        big.push_str(&"x".repeat(5 * 1024 * 1024));
        big.push_str(r#"<link rel="icon" href="/late.png">"#);
        assert_eq!(
            strings(&icon_candidates(&big, &base())),
            vec!["https://radio.example.org/favicon.ico"]
        );
        // An unclosed tag at the end, and multi-byte text right at the scan limit.
        let unclosed = format!("{}<link rel=\"icon\" href=\"/x.png", "ä".repeat(300_000));
        assert_eq!(icon_candidates(&unclosed, &base()).len(), 1);
        for tail in [
            "<",
            "<link",
            "<link rel=",
            "<link rel=\"",
            "<!--",
            "<script>",
            "</",
        ] {
            assert_eq!(icon_candidates(tail, &base()).len(), 1, "{tail}");
        }
        // 10 000 link tags.
        let many: String = (0..10_000)
            .map(|n| format!(r#"<link rel="icon" sizes="{n}x{n}" href="/i{n}.png">"#))
            .collect();
        let found = icon_candidates(&many, &base());
        assert!(found.len() <= MAX_CANDIDATES, "{}", found.len());
        assert_eq!(
            found.last().map(Url::as_str),
            Some("https://radio.example.org/favicon.ico")
        );
        // A huge tag without an end.
        let giant = format!("<link rel=\"icon\" href=\"/{}", "y".repeat(100_000));
        assert_eq!(icon_candidates(&giant, &base()).len(), 1);
    }

    // --- fetching -----------------------------------------------------------------------

    fn png() -> Vec<u8> {
        let image = RgbaImage::from_pixel(16, 16, Rgba([0x4d, 0xa6, 0xff, 0xff]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn ico() -> Vec<u8> {
        let image = RgbaImage::from_pixel(32, 32, Rgba([0xff, 0x4d, 0x6d, 0xff]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Ico).unwrap();
        out.into_inner()
    }

    struct Env {
        db: Arc<Db>,
        covers: FaviconCovers,
    }

    impl Env {
        fn new() -> Env {
            let db = Arc::new(Db::open_in_memory().unwrap());
            let covers = FaviconCovers::with_private(Arc::clone(&db), true).unwrap();
            Env { db, covers }
        }

        /// Remembers the logo of a station with this favicon and homepage; returns the raw
        /// part of its cover id.
        fn station(&self, favicon: Option<&str>, homepage: Option<&str>) -> String {
            let station = Station {
                favicon: favicon.map(str::to_owned),
                homepage: homepage.map(str::to_owned),
                ..station(1)
            };
            let conn = self.db.0.lock().unwrap();
            store::remember_favicons(&conn, &[station]).unwrap();
            let id = station_cover_id(favicon, homepage).unwrap();
            id[RADIO_COVER_PREFIX.len()..].to_owned()
        }

        fn missing_until(&self, raw: &str) -> Option<i64> {
            let conn = self.db.0.lock().unwrap();
            store::favicon_missing_until(&conn, &format!("{RADIO_COVER_PREFIX}{raw}")).unwrap()
        }
    }

    #[test]
    fn favicon_png_is_fetched() {
        let web = FakeWeb::start();
        web.set("/logo.png", Page::ok("image/png", png()));
        let env = Env::new();
        let raw = env.station(Some(&web.url("/logo.png")), None);
        assert_eq!(env.covers.fetch(&raw, 96), Ok(png()));
        assert_eq!(env.missing_until(&raw), None);
    }

    #[test]
    fn favicon_too_big_is_unknown() {
        let web = FakeWeb::start();
        let mut huge = png();
        huge.resize(2 * 1024 * 1024, 0);
        web.set("/sized.png", Page::ok("image/png", huge.clone()));
        huge.resize(3 * 1024 * 1024, 0);
        web.set("/unsized.png", Page::ok("image/png", huge).without_length());
        let env = Env::new();
        for path in ["/sized.png", "/unsized.png"] {
            let raw = env.station(Some(&web.url(path)), None);
            assert_eq!(
                env.covers.fetch(&raw, 96),
                Err(ResizeError::Unknown),
                "{path}"
            );
            assert!(env.missing_until(&raw).is_some());
        }
    }

    #[test]
    fn favicon_html_is_unknown_and_not_asked_again() {
        let web = FakeWeb::start();
        web.set(
            "/logo.png",
            Page::ok("text/html", "<html><body>Not here</body></html>"),
        );
        let env = Env::new();
        let raw = env.station(Some(&web.url("/logo.png")), None);
        let before = now_ms();
        assert_eq!(env.covers.fetch(&raw, 96), Err(ResizeError::Unknown));
        let until = env.missing_until(&raw).unwrap();
        assert!(until >= before + MISSING_FOR_MS && until <= now_ms() + MISSING_FOR_MS);
        assert_eq!(env.covers.fetch(&raw, 300), Err(ResizeError::Unknown));
        assert_eq!(web.hits("/logo.png"), 1, "asked once");
    }

    #[test]
    fn svg_favicon_is_unknown() {
        let web = FakeWeb::start();
        web.set(
            "/logo.svg",
            Page::ok(
                "image/svg+xml",
                r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
            ),
        );
        let env = Env::new();
        let raw = env.station(Some(&web.url("/logo.svg")), None);
        assert_eq!(env.covers.fetch(&raw, 96), Err(ResizeError::Unknown));
    }

    #[test]
    fn ico_favicon_decodes() {
        let web = FakeWeb::start();
        web.set("/favicon.ico", Page::ok("image/x-icon", ico()));
        let env = Env::new();
        let raw = env.station(Some(&web.url("/favicon.ico")), None);
        assert_eq!(env.covers.fetch(&raw, 96), Ok(ico()));
        // The gate encodes it like any cover.
        let dir = tempfile::tempdir().unwrap();
        let gate = ResizeGate::new(2);
        gate.set_remote_for(
            RADIO_COVER_PREFIX,
            Arc::new(FaviconCovers::with_private(Arc::clone(&env.db), true).unwrap()),
        );
        let store = crate::library::covers::CoverStore::new(dir.path().join("original"));
        let id = format!("{RADIO_COVER_PREFIX}{raw}");
        for size in SIZES {
            let path = gate.get(&store, dir.path(), &id, size).unwrap();
            let decoded = image::load_from_memory(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (32, 32));
        }
    }

    #[test]
    fn homepage_icon_used_without_favicon() {
        let web = FakeWeb::start();
        web.set(
            "/",
            Page::ok(
                "text/html; charset=utf-8",
                r#"<html><head><link rel="apple-touch-icon" href="/apple.png"></head></html>"#,
            ),
        );
        web.set("/apple.png", Page::ok("image/png", png()));
        let env = Env::new();
        let raw = env.station(None, Some(&web.url("/")));
        assert_eq!(env.covers.fetch(&raw, 96), Ok(png()));
        assert_eq!(web.hits("/favicon.ico"), 0, "the first candidate won");
    }

    #[test]
    fn homepage_used_when_favicon_fails() {
        let web = FakeWeb::start();
        web.set("/broken.png", Page::ok("image/png", "not an image"));
        web.set("/home", Page::redirect("/home/"));
        web.set(
            "/home/",
            Page::ok(
                "text/html",
                r#"<head><link rel="icon" href="logo.png"><link rel="icon" href="/a.svg"></head>"#,
            ),
        );
        // A relative icon resolves against the page after the redirect.
        web.set("/home/logo.png", Page::status(404));
        web.set("/favicon.ico", Page::ok("image/x-icon", ico()));
        let env = Env::new();
        let raw = env.station(Some(&web.url("/broken.png")), Some(&web.url("/home")));
        assert_eq!(env.covers.fetch(&raw, 96), Ok(ico()));
        assert_eq!(web.hits("/home/logo.png"), 1);
        assert_eq!(web.hits("/a.svg"), 0);
    }

    #[test]
    fn homepage_without_icons_falls_back_to_the_origin_favicon() {
        let web = FakeWeb::start();
        web.set("/page", Page::status(500));
        web.set("/favicon.ico", Page::ok("image/png", png()));
        let env = Env::new();
        let raw = env.station(None, Some(&web.url("/page")));
        assert_eq!(env.covers.fetch(&raw, 96), Ok(png()));
    }

    #[test]
    fn a_stream_as_homepage_gives_no_server_favicon() {
        let web = FakeWeb::start();
        web.set("/stream", Page::ok("audio/mpeg", "ID3"));
        web.set("/favicon.ico", Page::ok("image/x-icon", ico()));
        let env = Env::new();
        let raw = env.station(None, Some(&web.url("/stream")));
        assert_eq!(env.covers.fetch(&raw, 96), Err(ResizeError::Unknown));
        assert_eq!(web.hits("/favicon.ico"), 0);
    }

    #[test]
    fn redirects_are_capped() {
        let web = FakeWeb::start();
        web.set("/r1", Page::redirect("/r2"));
        web.set("/r2", Page::redirect("/r3"));
        web.set("/r3", Page::redirect("/logo.png"));
        web.set("/logo.png", Page::ok("image/png", png()));
        web.set("/s1", Page::redirect("/s2"));
        web.set("/s2", Page::redirect("/s3"));
        web.set("/s3", Page::redirect("/s4"));
        web.set("/s4", Page::redirect("/logo.png"));
        let env = Env::new();
        let three = env.station(Some(&web.url("/r1")), None);
        assert_eq!(env.covers.fetch(&three, 96), Ok(png()));
        let four = env.station(Some(&web.url("/s1")), None);
        assert_eq!(env.covers.fetch(&four, 96), Err(ResizeError::Unknown));
    }

    #[test]
    fn an_unreachable_host_is_not_remembered() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let env = Env::new();
        let raw = env.station(
            Some(&format!("http://127.0.0.1:{port}/logo.png")),
            Some(&format!("http://127.0.0.1:{port}/")),
        );
        assert_eq!(env.covers.fetch(&raw, 96), Err(ResizeError::Unreachable));
        assert_eq!(env.missing_until(&raw), None);
    }

    #[test]
    fn private_addresses_are_refused() {
        let web = FakeWeb::start();
        web.set("/logo.png", Page::ok("image/png", png()));
        web.set(
            "/",
            Page::ok("text/html", r#"<link rel="icon" href="/logo.png">"#),
        );
        let env = Env::new();
        let covers = FaviconCovers::new(Arc::clone(&env.db)).unwrap();
        let port = web
            .url("/")
            .split(':')
            .nth(2)
            .unwrap()
            .trim_end_matches('/')
            .to_owned();
        for (favicon, homepage) in [
            (Some(web.url("/logo.png")), None),
            (None, Some(web.url("/"))),
            // A host name that resolves to this machine.
            (Some(format!("http://localhost:{port}/logo.png")), None),
            (Some(format!("http://[::1]:{port}/logo.png")), None),
            (
                Some(format!("http://[::ffff:127.0.0.1]:{port}/logo.png")),
                None,
            ),
        ] {
            let raw = env.station(favicon.as_deref(), homepage.as_deref());
            assert_eq!(
                covers.fetch(&raw, 96),
                Err(ResizeError::Unknown),
                "{favicon:?} {homepage:?}"
            );
            assert!(env.missing_until(&raw).is_some(), "remembered as missing");
        }
        assert_eq!(
            web.hits("/logo.png") + web.hits("/") + web.hits("/favicon.ico"),
            0
        );
        // A redirect into a private network is refused like a first request.
        for target in [
            "http://127.0.0.1/logo.png",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://169.254.169.254/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://255.255.255.255/",
            "http://224.0.0.1/",
            "http://[::1]/",
            "http://[::]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[ff02::1]/",
            "http://[::ffff:10.0.0.1]/",
        ] {
            assert!(
                !url_allowed(&Url::parse(target).unwrap(), false),
                "{target}"
            );
            assert!(url_allowed(&Url::parse(target).unwrap(), true), "{target}");
        }
        for public in [
            "https://radio.example.org/",
            "http://93.184.216.34/",
            "http://[2001:db8::1]/",
        ] {
            assert!(url_allowed(&Url::parse(public).unwrap(), false), "{public}");
        }
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A PNG whose header claims `side` x `side` pixels, with no image data.
    fn png_claiming(side: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        };
        let mut header = side.to_be_bytes().to_vec();
        header.extend_from_slice(&side.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(b"IHDR", &header);
        chunk(b"IEND", &[]);
        out
    }

    #[test]
    fn decompression_bombs_are_unknown() {
        let bomb = png_claiming(100_000);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(bomb.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&bomb);
        let web = FakeWeb::start();
        web.set("/bomb.png", Page::ok("image/png", bomb));
        web.set("/bomb.ico", Page::ok("image/x-icon", ico));
        let env = Env::new();
        for path in ["/bomb.png", "/bomb.ico"] {
            let raw = env.station(Some(&web.url(path)), None);
            assert_eq!(
                env.covers.fetch(&raw, 96),
                Err(ResizeError::Unknown),
                "{path}"
            );
            assert_eq!(web.hits(path), 1);
        }
        // Refused by the dimension bound, before anything is allocated for the pixels.
        assert!(!decodes(&png_claiming(100_000)));
    }

    #[test]
    fn an_unknown_logo_makes_no_request() {
        let env = Env::new();
        assert_eq!(
            env.covers.fetch("0123456789abcdef0123", 96),
            Err(ResizeError::Unknown)
        );
    }
}
