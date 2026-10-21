//! A blocking Subsonic / OpenSubsonic client with token authentication.
//!
//! The password never leaves this module except as a salted token in a request's query. Every
//! error is built from a reqwest error stripped of its URL, and the password, token and salt of
//! the request are scrubbed from its text once more, so no error, log line or event can carry
//! them.

use std::error::Error as StdError;
use std::fmt;
use std::io::Read;
use std::time::Duration;

use anyhow::Context;
use md5::{Digest, Md5};
use rand::Rng;
use reqwest::blocking::{Client, Response};
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::secret::{Secret, SecretUrl};

/// The Subsonic API version Prism speaks.
pub const API_VERSION: &str = "1.16.1";
/// The `c` parameter of every request.
pub const CLIENT_NAME: &str = "Prism";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// Total time of one API call, first byte (a buffering reverse proxy can be slow) and body.
pub const API_TIMEOUT: Duration = Duration::from_secs(20);
/// Page size of the paged calls (`getAlbumList2`, `search3`).
pub const PAGE: u32 = 500;
pub const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
/// Redirects followed per request (as reqwest's default). See [`check_redirect`]. A reverse proxy commonly
/// redirects to its base path or from http to https.
const MAX_REDIRECTS: usize = 10;
/// TCP keepalive of the stream client, so a dead connection under a paused track is noticed.
const STREAM_KEEPALIVE: Duration = Duration::from_secs(15);
/// The stream client's timeout. In blocking reqwest it applies to each operation (the wait for
/// the headers, then every single body read), not to the whole response. It sits above the
/// stream buffer's stall timeout, so the reader side decides first; this bound only ends a
/// download thread that is left blocked on a silent server.
pub const STREAM_READ_TIMEOUT: Duration =
    Duration::from_secs(crate::audio::http::STALL_TIMEOUT.as_secs() + 10);

/// A Navidrome server and the user signed in to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Normalized by [`normalize_url`].
    pub url: String,
    pub user: String,
    pub password: Secret,
}

/// Everything that can go wrong talking to the server. No variant ever carries a URL, password,
/// token or salt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavidromeError {
    InvalidUrl(String),
    WrongCredentials,
    TokenAuthUnsupported,
    NotAuthorized,
    NotFound,
    Unreachable(String),
    Tls(String),
    NotSubsonic(String),
    Server { code: u32, message: String },
}

impl NavidromeError {
    /// Stable code for the frontend.
    pub fn code(&self) -> &'static str {
        match self {
            NavidromeError::InvalidUrl(_) => "invalidUrl",
            NavidromeError::WrongCredentials => "wrongCredentials",
            NavidromeError::TokenAuthUnsupported => "tokenAuthUnsupported",
            NavidromeError::NotAuthorized => "notAuthorized",
            NavidromeError::NotFound => "notFound",
            NavidromeError::Unreachable(_) => "unreachable",
            NavidromeError::Tls(_) => "tls",
            NavidromeError::NotSubsonic(_) => "notSubsonic",
            NavidromeError::Server { .. } => "server",
        }
    }

    /// True when the server cannot be reached right now (network down, server or its backend
    /// down); everything else is an answer from the server.
    pub fn is_offline(&self) -> bool {
        matches!(self, NavidromeError::Unreachable(_))
    }

    fn detail(&self) -> Option<&str> {
        match self {
            NavidromeError::InvalidUrl(detail)
            | NavidromeError::Unreachable(detail)
            | NavidromeError::Tls(detail)
            | NavidromeError::NotSubsonic(detail) => Some(detail),
            NavidromeError::Server { message, .. } => Some(message),
            _ => None,
        }
    }

    /// Replaces every occurrence of the given secrets in the error's text with `***`. Callers pass
    /// the password only when it is at least [`MIN_SCRUBBED_PASSWORD`] chars long (see
    /// [`scrub_secrets`]); token and salt always.
    fn scrub(self, secrets: &[&str]) -> NavidromeError {
        let clean = |mut text: String| {
            for secret in secrets.iter().filter(|s| !s.is_empty()) {
                text = text.replace(secret, "***");
            }
            text
        };
        match self {
            NavidromeError::InvalidUrl(d) => NavidromeError::InvalidUrl(clean(d)),
            NavidromeError::Unreachable(d) => NavidromeError::Unreachable(clean(d)),
            NavidromeError::Tls(d) => NavidromeError::Tls(clean(d)),
            NavidromeError::NotSubsonic(d) => NavidromeError::NotSubsonic(clean(d)),
            NavidromeError::Server { code, message } => NavidromeError::Server {
                code,
                message: clean(message),
            },
            other => other,
        }
    }
}

impl fmt::Display for NavidromeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NavidromeError::InvalidUrl(d) => write!(f, "invalid server address: {d}"),
            NavidromeError::WrongCredentials => f.write_str("wrong user name or password"),
            NavidromeError::TokenAuthUnsupported => {
                f.write_str("the server does not support token authentication")
            }
            NavidromeError::NotAuthorized => f.write_str("the user is not authorized for this"),
            NavidromeError::NotFound => f.write_str("not found on the server"),
            NavidromeError::Unreachable(d) => write!(f, "the server could not be reached: {d}"),
            NavidromeError::Tls(d) => write!(f, "TLS error: {d}"),
            NavidromeError::NotSubsonic(d) => write!(f, "not a Subsonic server: {d}"),
            NavidromeError::Server { code, message } => {
                write!(f, "server error {code}: {message}")
            }
        }
    }
}

impl StdError for NavidromeError {}

/// A failure as the frontend sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavidromeFailure {
    pub code: &'static str,
    pub detail: Option<String>,
}

impl From<&NavidromeError> for NavidromeFailure {
    fn from(err: &NavidromeError) -> NavidromeFailure {
        NavidromeFailure {
            code: err.code(),
            detail: err.detail().map(str::to_owned),
        }
    }
}

/// Trims the address, prepends `https://` when it has no scheme, accepts only `http` and
/// `https`, rejects a query, a fragment and credentials, drops the trailing `/` and keeps a path
/// prefix (a reverse proxy serving Navidrome under `/music`).
pub fn normalize_url(input: &str) -> Result<String, NavidromeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(NavidromeError::InvalidUrl("the address is empty".into()));
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&with_scheme)
        .map_err(|_| NavidromeError::InvalidUrl("not a valid address".into()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(NavidromeError::InvalidUrl(
            "only http and https are supported".into(),
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(NavidromeError::InvalidUrl("the address has no host".into()));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(NavidromeError::InvalidUrl(
            "the address must not have a query or fragment".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(NavidromeError::InvalidUrl(
            "the address must not contain a user or password".into(),
        ));
    }
    // Users paste the API root (`.../rest`); calls append `/rest/<method>` themselves.
    let mut url = url;
    let path = url.path().trim_end_matches('/').to_owned();
    if let Some(prefix) = path.strip_suffix("/rest") {
        url.set_path(prefix);
    }
    let mut normalized = url.to_string();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

/// The Subsonic token: lowercase hex md5(password + salt).
pub(crate) fn token(password: &str, salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    hex::encode(hasher.finalize())
}

/// A fresh 12-hex-char salt.
fn new_salt() -> String {
    let bytes: [u8; 6] = rand::rng().random();
    hex::encode(bytes)
}

/// Stream quality: the original file or an MP3 transcode at a bitrate cap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamQuality {
    #[default]
    Original,
    #[serde(rename = "320")]
    Kbps320,
    #[serde(rename = "192")]
    Kbps192,
    #[serde(rename = "128")]
    Kbps128,
}

impl StreamQuality {
    /// The `maxBitRate` of a transcode, `None` for the original.
    pub fn max_bitrate(self) -> Option<u32> {
        match self {
            StreamQuality::Original => None,
            StreamQuality::Kbps320 => Some(320),
            StreamQuality::Kbps192 => Some(192),
            StreamQuality::Kbps128 => Some(128),
        }
    }
}

/// An artist reference inside a song (OpenSubsonic `albumArtists`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
}

/// OpenSubsonic ReplayGain of a song, in dB.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplayGainInfo {
    pub track_gain: Option<f64>,
    pub album_gain: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_peak: Option<f64>,
    pub base_gain: Option<f64>,
    pub fallback_gain: Option<f64>,
}

/// A song (`Child` in the Subsonic schema).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Child {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub cover_art: Option<String>,
    pub suffix: Option<String>,
    pub content_type: Option<String>,
    /// Seconds.
    pub duration: Option<u32>,
    /// kbit/s.
    pub bit_rate: Option<u32>,
    pub sampling_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub display_album_artist: Option<String>,
    pub album_artists: Vec<ArtistRef>,
    pub replay_gain: Option<ReplayGainInfo>,
    /// ISO timestamp of the star, `None` when not starred.
    pub starred: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlbumId3 {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub cover_art: Option<String>,
    pub song_count: u32,
    /// Seconds.
    pub duration: u32,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub starred: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArtistId3 {
    pub id: String,
    pub name: String,
    pub starred: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Starred2 {
    pub song: Vec<Child>,
    pub album: Vec<AlbumId3>,
    pub artist: Vec<ArtistId3>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub song_count: u32,
    /// Seconds.
    pub duration: u32,
    pub changed: Option<String>,
    pub cover_art: Option<String>,
}

/// A playlist with its songs: the summary fields plus `entry`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaylistWithSongs {
    #[serde(flatten)]
    pub summary: PlaylistSummary,
    pub entry: Vec<Child>,
}

/// What `ping` tells about the server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub server_type: Option<String>,
    pub server_version: Option<String>,
    pub api_version: String,
    pub open_subsonic: bool,
}

/// A star or unstar target, by raw server id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StarTarget {
    Song(String),
    Album(String),
    Artist(String),
}

impl StarTarget {
    fn param(&self) -> (&'static str, &str) {
        match self {
            StarTarget::Song(id) => ("id", id),
            StarTarget::Album(id) => ("albumId", id),
            StarTarget::Artist(id) => ("artistId", id),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Indexes {
    last_modified: i64,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AlbumList2 {
    album: Vec<AlbumId3>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AlbumWithSongs {
    #[serde(flatten)]
    album: AlbumId3,
    song: Vec<Child>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct SearchResult3 {
    song: Vec<Child>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Playlists {
    playlist: Vec<PlaylistSummary>,
}

/// Maps a Subsonic error code.
fn from_code(code: u32, message: String) -> NavidromeError {
    match code {
        40 => NavidromeError::WrongCredentials,
        41..=44 => NavidromeError::TokenAuthUnsupported,
        50 => NavidromeError::NotAuthorized,
        70 => NavidromeError::NotFound,
        _ => NavidromeError::Server { code, message },
    }
}

/// Maps a non-2xx response. A reverse proxy answers 502 / 503 / 504 while Navidrome behind it
/// is down: that is the server being unreachable, not a foreign server. A 401 is
/// `WrongCredentials` only when its body is a Subsonic response; a 401 from the proxy's own
/// authentication (an HTML or empty body) is `NotSubsonic("HTTP 401")`.
fn status_error(response: Response) -> NavidromeError {
    let status = response.status().as_u16();
    match status {
        401 => {
            let body = response.bytes().unwrap_or_default();
            match envelope(&body) {
                Ok(_) => NavidromeError::WrongCredentials,
                Err(NavidromeError::NotSubsonic(_)) => {
                    NavidromeError::NotSubsonic("HTTP 401".into())
                }
                Err(err) => err,
            }
        }
        502..=504 => NavidromeError::Unreachable(format!("HTTP {status}")),
        n => NavidromeError::NotSubsonic(format!("HTTP {n}")),
    }
}

/// Why a redirect was not followed. Carries no URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RedirectRefusal {
    OtherHost,
    Downgrade,
    TooMany,
}

impl fmt::Display for RedirectRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RedirectRefusal::OtherHost => "redirect to another host",
            RedirectRefusal::Downgrade => "redirect to http",
            RedirectRefusal::TooMany => "too many redirects",
        })
    }
}

impl StdError for RedirectRefusal {}

/// Whether a redirect to `next` may be followed: only to the account's host (any port), never
/// from https down to http, at most [`MAX_REDIRECTS`] hops. The credentials ride in the query,
/// so a redirect elsewhere would hand them to another server.
fn check_redirect(host: &str, previous: &[Url], next: &Url) -> Result<(), RedirectRefusal> {
    // `previous` holds the original URL and every URL already redirected to, not `next`: with
    // 1 + 9 entries this is the 10th redirect (followed), with 1 + 10 the 11th (refused). The
    // same count as reqwest's `Policy::limited(10)`.
    if previous.len() > MAX_REDIRECTS {
        return Err(RedirectRefusal::TooMany);
    }
    if !next
        .host_str()
        .is_some_and(|h| h.eq_ignore_ascii_case(host))
    {
        return Err(RedirectRefusal::OtherHost);
    }
    if next.scheme() != "https" && previous.iter().any(|u| u.scheme() == "https") {
        return Err(RedirectRefusal::Downgrade);
    }
    Ok(())
}

fn redirect_policy(host: String) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        match check_redirect(&host, attempt.previous(), attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(refusal) => attempt.error(refusal),
        }
    })
}

/// Passwords shorter than this are not scrubbed from error text: replacing every "a" or "12"
/// would garble the message and hide nothing worth hiding.
const MIN_SCRUBBED_PASSWORD: usize = 4;

/// The secrets to scrub from an error of one request.
fn scrub_secrets<'a>(password: &'a str, token: &'a str, salt: &'a str) -> Vec<&'a str> {
    let mut secrets = vec![token, salt];
    if password.chars().count() >= MIN_SCRUBBED_PASSWORD {
        secrets.push(password);
    }
    secrets
}

/// Replaces every word that looks like a URL or carries a query parameter with `<redacted>`.
/// A second line of defense behind `without_url()`, for URLs nested in an error chain.
fn redact_urls(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let query = word.trim_end_matches(['?', ')', '.', ',']).contains('?');
            if word.contains("://") || word.contains('=') || query {
                "<redacted>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text of an error and its sources, without repeating a source its parent already prints.
fn chain_text(err: &(dyn StdError + 'static)) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(inner) = source {
        let part = inner.to_string();
        if !part.is_empty() && !text.contains(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = inner.source();
    }
    text
}

/// Classifies a transport error. A chain that mentions a certificate is `Tls`; a refused or
/// looping redirect and an undecodable body (not a timeout) are `NotSubsonic`; everything else
/// (connect, DNS, timeout, a dropped connection) is `Unreachable`. Words that look like URLs or
/// query parameters are redacted from the text. For a reqwest error use [`classify_reqwest`].
pub(crate) fn classify(err: &(dyn StdError + 'static)) -> NavidromeError {
    let text = redact_urls(&chain_text(err));
    if text.to_lowercase().contains("certificate") {
        return NavidromeError::Tls(text);
    }
    let mut current = Some(err);
    while let Some(inner) = current {
        if let Some(refusal) = inner.downcast_ref::<RedirectRefusal>() {
            return NavidromeError::NotSubsonic(refusal.to_string());
        }
        current = inner.source();
    }
    let mut current = Some(err);
    while let Some(inner) = current {
        if let Some(reqwest_err) = inner.downcast_ref::<reqwest::Error>() {
            if reqwest_err.is_redirect() {
                return NavidromeError::NotSubsonic("too many redirects".into());
            }
            // reqwest wraps every body error (a cut connection too) in Kind::Decode; only
            // invalid data (a broken gzip stream) means the server answered garbage.
            if reqwest_err.is_decode() && !reqwest_err.is_timeout() && has_invalid_data(err) {
                return NavidromeError::NotSubsonic("the response could not be decoded".into());
            }
        }
        current = inner.source();
    }
    NavidromeError::Unreachable(text)
}

/// True when an `io::Error` of kind `InvalidData` is in the chain.
fn has_invalid_data(err: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(inner) = current {
        if inner
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::InvalidData)
        {
            return true;
        }
        current = inner.source();
    }
    false
}

/// The only safe way to turn a reqwest error into a [`NavidromeError`]: strips the request URL
/// (which carries the token and salt), redacts anything URL-like left in the chain, then
/// classifies. Stream code (Task 7) must use this, never `to_string()` of a reqwest error.
pub(crate) fn classify_reqwest(err: reqwest::Error) -> NavidromeError {
    let err = err.without_url();
    classify(&err)
}

/// Classifies an I/O error from reading a response body; a reqwest error inside it goes through
/// [`classify_reqwest`].
pub(crate) fn classify_io(err: std::io::Error) -> NavidromeError {
    if err
        .get_ref()
        .is_some_and(|inner| inner.is::<reqwest::Error>())
    {
        if let Some(inner) = err.into_inner() {
            if let Ok(reqwest_err) = inner.downcast::<reqwest::Error>() {
                return classify_reqwest(*reqwest_err);
            }
        }
        return NavidromeError::Unreachable("the connection failed".into());
    }
    classify(&err)
}

/// Parses a response body into the inner `subsonic-response` object, mapping a failed status.
fn envelope(body: &[u8]) -> Result<Value, NavidromeError> {
    let not_subsonic = || NavidromeError::NotSubsonic("not a Subsonic server".into());
    let mut value: Value = serde_json::from_slice(body).map_err(|_| not_subsonic())?;
    let response = value
        .get_mut("subsonic-response")
        .filter(|r| r.is_object())
        .map(Value::take)
        .ok_or_else(not_subsonic)?;
    if response.get("status").and_then(Value::as_str) == Some("failed") {
        let error = response.get("error");
        let code = error
            .and_then(|e| e.get("code"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        return Err(from_code(u32::try_from(code).unwrap_or(0), message));
    }
    Ok(response)
}

/// Deserializes one key of a `subsonic-response`; a missing or null key is the type's default.
fn field<T: DeserializeOwned + Default>(
    response: &mut Value,
    key: &str,
) -> Result<T, NavidromeError> {
    match response.get_mut(key).map(Value::take) {
        None | Some(Value::Null) => Ok(T::default()),
        Some(value) => serde_json::from_value(value)
            .map_err(|_| NavidromeError::NotSubsonic(format!("unexpected {key} response"))),
    }
}

/// [`field`] that refuses a missing or null `key`.
fn required_field<T: DeserializeOwned + Default>(
    response: &mut Value,
    key: &str,
) -> Result<T, NavidromeError> {
    match response.get(key) {
        None | Some(Value::Null) => Err(NavidromeError::NotSubsonic(format!(
            "unexpected {key} response"
        ))),
        Some(_) => field(response, key),
    }
}

/// A blocking client for one account.
pub struct SubsonicClient {
    account: Account,
    http: Client,
    stream: Client,
}

impl SubsonicClient {
    pub fn new(account: Account) -> anyhow::Result<SubsonicClient> {
        SubsonicClient::with_timeouts(account, CONNECT_TIMEOUT, API_TIMEOUT)
    }

    pub(crate) fn with_timeouts(
        account: Account,
        connect: Duration,
        api: Duration,
    ) -> anyhow::Result<SubsonicClient> {
        SubsonicClient::with_stream_timeout(account, connect, api, STREAM_READ_TIMEOUT)
    }

    /// Like [`Self::with_timeouts`], with the stream client's per-read timeout too.
    pub(crate) fn with_stream_timeout(
        account: Account,
        connect: Duration,
        api: Duration,
        stream_read: Duration,
    ) -> anyhow::Result<SubsonicClient> {
        let host = Url::parse(&account.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .context("the server address is not a valid URL")?;
        let user_agent = format!("{CLIENT_NAME}/{}", env!("CARGO_PKG_VERSION"));
        let http = Client::builder()
            .user_agent(&user_agent)
            .connect_timeout(connect)
            .timeout(api)
            .redirect(redirect_policy(host.clone()))
            .build()
            .context("building the HTTP client")?;
        // Streams: no bound on the whole response (a track streams for minutes), only one per
        // operation above the stall timeout (a buffering proxy can be slow to the first byte;
        // a silent server must not hold a download thread forever), keepalive to notice a dead
        // connection, no transparent decompression (byte offsets must stay the file's).
        let stream = Client::builder()
            .user_agent(&user_agent)
            .connect_timeout(connect)
            .timeout(stream_read)
            .tcp_keepalive(STREAM_KEEPALIVE)
            .no_gzip()
            .redirect(redirect_policy(host))
            .build()
            .context("building the stream HTTP client")?;
        Ok(SubsonicClient {
            account,
            http,
            stream,
        })
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    fn method_url(&self, method: &str) -> String {
        format!("{}/rest/{method}", self.account.url)
    }

    /// The authentication parameters with a fresh salt, and the salt and token for scrubbing.
    fn auth(&self) -> (Vec<(&'static str, String)>, String, String) {
        let salt = new_salt();
        let token = token(self.account.password.expose(), &salt);
        let params = vec![
            ("u", self.account.user.clone()),
            ("t", token.clone()),
            ("s", salt.clone()),
            ("v", API_VERSION.to_owned()),
            ("c", CLIENT_NAME.to_owned()),
            ("f", "json".to_owned()),
        ];
        (params, salt, token)
    }

    /// Sends one request and hands the response to `read`. Every error leaves with the password,
    /// token and salt of this request scrubbed from its text.
    fn request<T>(
        &self,
        method: &str,
        params: &[(&str, String)],
        read: impl FnOnce(Response) -> Result<T, NavidromeError>,
    ) -> Result<T, NavidromeError> {
        let (auth, salt, token) = self.auth();
        let result = self
            .http
            .get(self.method_url(method))
            .query(&auth)
            .query(params)
            .send()
            .map_err(classify_reqwest)
            .and_then(read);
        result.map_err(|err| {
            err.scrub(&scrub_secrets(
                self.account.password.expose(),
                &token,
                &salt,
            ))
        })
    }

    /// One API call: a 2xx JSON `subsonic-response` with status `ok`.
    fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value, NavidromeError> {
        self.request(method, params, |response| {
            if !response.status().is_success() {
                return Err(status_error(response));
            }
            let body = response.bytes().map_err(classify_reqwest)?;
            envelope(&body)
        })
    }

    pub fn ping(&self) -> Result<ServerInfo, NavidromeError> {
        let response = self.call("ping", &[])?;
        let text = |key: &str| response.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(ServerInfo {
            server_type: text("type"),
            server_version: text("serverVersion"),
            api_version: text("version").unwrap_or_default(),
            open_subsonic: response
                .get("openSubsonic")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// `getIndexes` `lastModified` (ms), the library change marker.
    pub fn indexes_last_modified(&self) -> Result<i64, NavidromeError> {
        let mut response = self.call("getIndexes", &[])?;
        let indexes: Indexes = field(&mut response, "indexes")?;
        Ok(indexes.last_modified)
    }

    /// One page of all albums by name.
    pub fn album_list2(&self, offset: u32, size: u32) -> Result<Vec<AlbumId3>, NavidromeError> {
        let mut response = self.call(
            "getAlbumList2",
            &[
                ("type", "alphabeticalByName".to_owned()),
                ("size", size.to_string()),
                ("offset", offset.to_string()),
            ],
        )?;
        let list: AlbumList2 = field(&mut response, "albumList2")?;
        Ok(list.album)
    }

    pub fn album(&self, id: &str) -> Result<(AlbumId3, Vec<Child>), NavidromeError> {
        let mut response = self.call("getAlbum", &[("id", id.to_owned())])?;
        let album: AlbumWithSongs = field(&mut response, "album")?;
        Ok((album.album, album.song))
    }

    /// One page of all songs (`search3` with an empty query).
    pub fn search3_songs(&self, offset: u32, count: u32) -> Result<Vec<Child>, NavidromeError> {
        let mut response = self.call(
            "search3",
            &[
                ("query", String::new()),
                ("artistCount", "0".to_owned()),
                ("albumCount", "0".to_owned()),
                ("songCount", count.to_string()),
                ("songOffset", offset.to_string()),
            ],
        )?;
        let result: SearchResult3 = field(&mut response, "searchResult3")?;
        Ok(result.song)
    }

    /// The server's stars. Strict: the import makes this the truth, so an ok envelope without
    /// `starred2` is an error, while an empty `starred2: {}` means nothing is starred.
    pub fn starred2(&self) -> Result<Starred2, NavidromeError> {
        let mut response = self.call("getStarred2", &[])?;
        required_field(&mut response, "starred2")
    }

    pub fn star(&self, target: &StarTarget, on: bool) -> Result<(), NavidromeError> {
        let (key, id) = target.param();
        self.call(if on { "star" } else { "unstar" }, &[(key, id.to_owned())])?;
        Ok(())
    }

    pub fn scrobble(&self, id: &str, time_ms: i64, submission: bool) -> Result<(), NavidromeError> {
        self.call(
            "scrobble",
            &[
                ("id", id.to_owned()),
                ("time", time_ms.to_string()),
                ("submission", submission.to_string()),
            ],
        )?;
        Ok(())
    }

    pub fn playlists(&self) -> Result<Vec<PlaylistSummary>, NavidromeError> {
        let mut response = self.call("getPlaylists", &[])?;
        let playlists: Playlists = field(&mut response, "playlists")?;
        Ok(playlists.playlist)
    }

    pub fn playlist(&self, id: &str) -> Result<PlaylistWithSongs, NavidromeError> {
        let mut response = self.call("getPlaylist", &[("id", id.to_owned())])?;
        field(&mut response, "playlist")
    }

    /// The cover image bytes, at most [`MAX_COVER_BYTES`].
    pub fn cover_art(&self, id: &str, size: u32) -> Result<Vec<u8>, NavidromeError> {
        self.request(
            "getCoverArt",
            &[("id", id.to_owned()), ("size", size.to_string())],
            |response| {
                let status = response.status();
                if status == StatusCode::NOT_FOUND {
                    return Err(NavidromeError::NotFound);
                }
                if !status.is_success() {
                    return Err(status_error(response));
                }
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                // One byte past the cap tells an oversized image from one of exactly the cap.
                let mut bytes = Vec::new();
                response
                    .take(MAX_COVER_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(classify_io)?;
                if content_type.contains("json") {
                    envelope(&bytes)?;
                    return Err(NavidromeError::NotSubsonic("expected an image".into()));
                }
                if content_type.starts_with("text/") {
                    return Err(NavidromeError::NotSubsonic("expected an image".into()));
                }
                if bytes.len() as u64 > MAX_COVER_BYTES {
                    return Err(NavidromeError::NotSubsonic("the cover is too large".into()));
                }
                Ok(bytes)
            },
        )
    }

    /// The stream URL of a song, with a fresh token. Only for [`Self::stream_http`] requests;
    /// never log it, never hand it to FFmpeg.
    pub fn stream_url(
        &self,
        id: &str,
        quality: StreamQuality,
        time_offset_s: Option<u32>,
    ) -> SecretUrl {
        let (auth, _, _) = self.auth();
        let mut url = Url::parse(&self.method_url("stream")).expect("the account URL was checked");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("id", id);
            for (key, value) in &auth {
                query.append_pair(key, value);
            }
            match quality.max_bitrate() {
                None => {
                    query.append_pair("format", "raw");
                }
                Some(rate) => {
                    query.append_pair("format", "mp3");
                    query.append_pair("maxBitRate", &rate.to_string());
                }
            }
            query.append_pair("estimateContentLength", "false");
            if let Some(offset) = time_offset_s {
                query.append_pair("timeOffset", &offset.to_string());
            }
        }
        SecretUrl::new(String::from(url))
    }

    /// The HTTP client for streams (built once): the connect timeout and keepalive, a per-read
    /// timeout ([`STREAM_READ_TIMEOUT`]) but no bound on the whole response, no transparent
    /// decompression, the same redirect rules as API calls. Map its errors with
    /// [`classify_reqwest`].
    pub fn stream_http(&self) -> reqwest::blocking::Client {
        self.stream.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::navidrome::testserver::{
        FakeLibrary, FakeServer, Fault, Redirect, FAKE_PASSWORD, FAKE_USER,
    };

    fn client(server: &FakeServer) -> SubsonicClient {
        SubsonicClient::new(server.account()).unwrap()
    }

    fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// A port nothing listens on.
    fn dead_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn token_matches_the_subsonic_example() {
        assert_eq!(
            token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }

    #[test]
    fn salt_is_fresh_per_request() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let client = client(&server);
        client.ping().unwrap();
        client.ping().unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let salts: Vec<&str> = requests
            .iter()
            .map(|r| param(&r.params, "s").unwrap())
            .collect();
        assert_ne!(salts[0], salts[1]);
        for request in &requests {
            let salt = param(&request.params, "s").unwrap();
            assert_eq!(salt.len(), 12);
            assert!(salt.chars().all(|c| c.is_ascii_hexdigit()));
            assert_eq!(param(&request.params, "u"), Some(FAKE_USER));
            assert_eq!(param(&request.params, "v"), Some(API_VERSION));
            assert_eq!(param(&request.params, "c"), Some(CLIENT_NAME));
            assert_eq!(param(&request.params, "f"), Some("json"));
            assert!(param(&request.params, "p").is_none());
        }
    }

    #[test]
    fn normalize_url_cases() {
        assert_eq!(normalize_url("nas:4533").unwrap(), "https://nas:4533");
        assert_eq!(normalize_url("  nas:4533  ").unwrap(), "https://nas:4533");
        assert_eq!(
            normalize_url("http://nas:4533/").unwrap(),
            "http://nas:4533"
        );
        assert_eq!(
            normalize_url("https://host/music/").unwrap(),
            "https://host/music"
        );
        assert_eq!(
            normalize_url("music.example").unwrap(),
            "https://music.example"
        );
        for bad in [
            "ftp://x",
            "",
            "   ",
            "https://h/?a=1",
            "https://h/#x",
            "https://demo:demo-pass@h",
        ] {
            assert!(
                matches!(normalize_url(bad), Err(NavidromeError::InvalidUrl(_))),
                "{bad:?} should be rejected"
            );
        }
        assert_eq!(
            normalize_url("https://host/music/rest").unwrap(),
            "https://host/music"
        );
        assert_eq!(
            normalize_url("https://host/music/rest/").unwrap(),
            "https://host/music"
        );
        assert_eq!(normalize_url("https://host/rest").unwrap(), "https://host");
        // Only one segment, and only a whole one.
        assert_eq!(
            normalize_url("https://host/rest/rest").unwrap(),
            "https://host/rest"
        );
        assert_eq!(
            normalize_url("https://host/forest").unwrap(),
            "https://host/forest"
        );
        assert_eq!(normalize_url("https://rest").unwrap(), "https://rest");
        assert_eq!(normalize_url("[::1]:4533").unwrap(), "https://[::1]:4533");
        let err = normalize_url("https://demo:demo-pass@h").unwrap_err();
        assert!(!err.to_string().contains("demo-pass"));
    }

    #[test]
    fn ping_reads_server_info() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let info = client(&server).ping().unwrap();
        assert_eq!(info.server_type.as_deref(), Some("navidrome"));
        assert!(info.server_version.is_some());
        assert_eq!(info.api_version, API_VERSION);
        assert!(info.open_subsonic);
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("serverType").is_some());
        assert!(json.get("openSubsonic").is_some());
    }

    #[test]
    fn base_url_keeps_a_path_prefix() {
        let server = FakeServer::start_at(FakeLibrary::synthetic(1, 1), "/music");
        assert!(server.url().ends_with("/music"));
        let client = client(&server);
        client.ping().unwrap();
        assert_eq!(client.album_list2(0, PAGE).unwrap().len(), 1);
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn redirects_are_followed() {
        for status in [301, 302, 307, 308] {
            let server = FakeServer::start_at(FakeLibrary::synthetic(2, 3), "/music");
            server.set_redirect(Some(Redirect::ToPrefix(status)));
            // The user typed the bare host; the proxy redirects to the base path.
            let root = server.url().trim_end_matches("/music").to_owned();
            let account = Account {
                url: root,
                ..server.account()
            };
            let client = SubsonicClient::new(account).unwrap();
            let info = client.ping().unwrap();
            assert_eq!(info.api_version, API_VERSION, "HTTP {status}");
            assert_eq!(
                client.album_list2(0, PAGE).unwrap().len(),
                2,
                "HTTP {status}"
            );
            let requests = server.requests();
            assert_eq!(requests.len(), 2, "HTTP {status}");
            assert!(param(&requests[0].params, "t").is_some());
        }
    }

    #[test]
    fn redirect_errors_carry_no_credentials() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let client = client(&server);

        // A redirect to a dead host: the target URL (with the query) must not leak.
        server.set_redirect(Some(Redirect::To(
            307,
            format!("http://127.0.0.1:{}", dead_port()),
        )));
        let err = client.ping().unwrap_err();
        assert!(err.is_offline(), "{err:?}");
        assert_clean(&err, &server);

        // A redirect loop.
        server.set_redirect(Some(Redirect::To(302, server.url())));
        let err = client.ping().unwrap_err();
        assert_eq!(
            err,
            NavidromeError::NotSubsonic("too many redirects".into())
        );
        assert_clean(&err, &server);
    }

    #[test]
    fn redirect_to_another_host_is_refused() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let port = server.url().rsplit(':').next().unwrap().to_owned();
        // Same port, other host name: the token must not follow it.
        server.set_redirect(Some(Redirect::To(302, format!("http://localhost:{port}"))));
        let client = client(&server);
        let err = client.ping().unwrap_err();
        assert_eq!(
            err,
            NavidromeError::NotSubsonic("redirect to another host".into())
        );
        assert!(!err.is_offline());
        assert_clean(&err, &server);
        // Nothing reached a method: the redirect was not followed.
        assert!(server.requests().is_empty());

        // The stream client follows the same rules.
        let url = client.stream_url("tr-0", StreamQuality::Original, None);
        let err = client.stream_http().get(url.expose()).send().unwrap_err();
        let err = classify_reqwest(err);
        assert_eq!(
            err,
            NavidromeError::NotSubsonic("redirect to another host".into())
        );
        assert_clean(&err, &server);
    }

    #[test]
    fn redirect_policy_decisions() {
        let url = |s: &str| Url::parse(s).unwrap();
        let from_https = [url("https://music.example/rest/ping")];
        let from_http = [url("http://music.example/rest/ping")];
        // Same host, other port or path: followed.
        assert_eq!(
            check_redirect(
                "music.example",
                &from_https,
                &url("https://music.example:8443/music/rest/ping")
            ),
            Ok(())
        );
        // Upgrade to https: followed.
        assert_eq!(
            check_redirect(
                "music.example",
                &from_http,
                &url("https://music.example/rest/ping")
            ),
            Ok(())
        );
        // Downgrade to http, also later in the chain: refused.
        assert_eq!(
            check_redirect(
                "music.example",
                &from_https,
                &url("http://music.example/rest/ping")
            ),
            Err(RedirectRefusal::Downgrade)
        );
        let chain = [
            url("https://music.example/rest/ping"),
            url("https://music.example/music/rest/ping"),
        ];
        assert_eq!(
            check_redirect(
                "music.example",
                &chain,
                &url("http://music.example/x/rest/ping")
            ),
            Err(RedirectRefusal::Downgrade)
        );
        // Another host: refused.
        assert_eq!(
            check_redirect(
                "music.example",
                &from_https,
                &url("https://evil.example/rest/ping")
            ),
            Err(RedirectRefusal::OtherHost)
        );
        // Hop limit.
        let many: Vec<Url> = (0..=MAX_REDIRECTS)
            .map(|i| url(&format!("https://music.example/{i}")))
            .collect();
        assert_eq!(
            check_redirect("music.example", &many, &url("https://music.example/x")),
            Err(RedirectRefusal::TooMany)
        );
        assert_eq!(
            check_redirect(
                "music.example",
                &many[..MAX_REDIRECTS],
                &url("https://music.example/x")
            ),
            Ok(()),
            "the 10th redirect is followed"
        );
        assert_eq!(RedirectRefusal::Downgrade.to_string(), "redirect to http");
    }

    #[test]
    fn api_body_cut_maps_to_unreachable() {
        for gzip in [false, true] {
            let server = FakeServer::start(FakeLibrary::synthetic(20, 10));
            server.set_gzip(gzip);
            let client = client(&server);

            server.set_fault(Fault::CutOnce {
                method: "ping",
                after_bytes: 5,
            });
            let err = client.ping().unwrap_err();
            assert!(err.is_offline(), "gzip {gzip}: {err:?}");
            assert_clean(&err, &server);

            server.set_fault(Fault::CutOnce {
                method: "search3",
                after_bytes: 50,
            });
            let err = client.search3_songs(0, PAGE).unwrap_err();
            assert!(err.is_offline(), "gzip {gzip}: {err:?}");
            assert_clean(&err, &server);

            // The cut was once: the next call goes through.
            assert_eq!(client.search3_songs(0, PAGE).unwrap().len(), 200);
            assert_eq!(server.gzipped_responses() > 0, gzip);
        }
    }

    #[test]
    fn stream_body_cut_maps_to_unreachable() {
        let lib = FakeLibrary::synthetic(1, 1)
            .with_audio("tr-0", (0..=255u8).cycle().take(10_000).collect());
        let server = FakeServer::start(lib);
        server.set_fault(Fault::CutOnce {
            method: "stream",
            after_bytes: 100,
        });
        let client = client(&server);
        let url = client.stream_url("tr-0", StreamQuality::Original, None);
        let response = client.stream_http().get(url.expose()).send().unwrap();
        let err = classify_reqwest(response.bytes().unwrap_err());
        assert!(err.is_offline(), "{err:?}");
        assert_clean(&err, &server);
    }

    #[test]
    fn corrupt_gzip_maps_to_not_subsonic() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        server.set_corrupt_gzip(true);
        let err = client(&server).ping().unwrap_err();
        assert_eq!(
            err,
            NavidromeError::NotSubsonic("the response could not be decoded".into())
        );
        assert_clean(&err, &server);
    }

    #[test]
    fn short_passwords_are_not_scrubbed() {
        assert_eq!(scrub_secrets("abc", "tok", "salt"), ["tok", "salt"]);
        assert_eq!(
            scrub_secrets("demo-pass", "tok", "salt"),
            ["tok", "salt", "demo-pass"]
        );
        let err = NavidromeError::Unreachable("an error near tok and salt".into());
        assert_eq!(
            err.scrub(&scrub_secrets("an", "tok", "salt")),
            NavidromeError::Unreachable("an error near *** and ***".into())
        );
    }

    #[test]
    fn redact_urls_hides_url_like_words() {
        assert_eq!(
            redact_urls("error for url (http://h/rest/ping?u=demo&t=abc&s=def): refused"),
            "error for url <redacted> refused"
        );
        assert_eq!(
            redact_urls("tcp connect error (os error 10061)"),
            "tcp connect error (os error 10061)"
        );
    }

    #[test]
    fn wrong_password_maps_to_wrong_credentials() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        server.set_fault(Fault::WrongPassword);
        let err = client(&server).ping().unwrap_err();
        assert_eq!(err, NavidromeError::WrongCredentials);
        assert_eq!(err.code(), "wrongCredentials");

        // A real wrong password, checked by the server's token validation.
        server.set_fault(Fault::None);
        let account = Account {
            password: Secret::new("not-the-password"),
            ..server.account()
        };
        let err = SubsonicClient::new(account).unwrap().ping().unwrap_err();
        assert_eq!(err, NavidromeError::WrongCredentials);
    }

    #[test]
    fn http_401_maps_by_body() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        // A Subsonic error body with 401: the server's verdict.
        server.set_fault(Fault::StatusWithCode {
            status: 401,
            code: 40,
        });
        assert_eq!(
            client(&server).ping().unwrap_err(),
            NavidromeError::WrongCredentials
        );
        // The proxy's own authentication (HTML body): not the Navidrome password.
        server.set_fault(Fault::Status(401));
        assert_eq!(
            client(&server).ping().unwrap_err(),
            NavidromeError::NotSubsonic("HTTP 401".into())
        );
        assert_eq!(
            client(&server).cover_art("al-0_cover", 64).unwrap_err(),
            NavidromeError::NotSubsonic("HTTP 401".into())
        );
    }

    #[test]
    fn code_41_maps_to_token_auth_unsupported() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        for code in [41, 42, 43, 44] {
            server.set_fault(Fault::Code(code));
            assert_eq!(
                client(&server).ping().unwrap_err(),
                NavidromeError::TokenAuthUnsupported
            );
        }
        server.set_fault(Fault::Code(50));
        assert_eq!(
            client(&server).ping().unwrap_err(),
            NavidromeError::NotAuthorized
        );
        server.set_fault(Fault::Code(70));
        assert_eq!(
            client(&server).ping().unwrap_err(),
            NavidromeError::NotFound
        );
        server.set_fault(Fault::Code(30));
        let err = client(&server).ping().unwrap_err();
        assert!(
            matches!(err, NavidromeError::Server { code: 30, .. }),
            "{err:?}"
        );
        assert_eq!(err.code(), "server");
    }

    #[test]
    fn refused_maps_to_unreachable() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let client = client(&server);
        server.set_fault(Fault::Refuse);
        let err = client.ping().unwrap_err();
        assert!(matches!(err, NavidromeError::Unreachable(_)), "{err:?}");
        assert!(err.is_offline());
        assert_eq!(err.code(), "unreachable");

        server.set_fault(Fault::None);
        client.ping().unwrap();

        server.stop();
        assert!(client.ping().unwrap_err().is_offline());
        server.restart();
        client.ping().unwrap();
    }

    #[test]
    fn timeout_maps_to_unreachable() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        server.set_fault(Fault::Stall {
            method: "ping",
            after_bytes: 0,
            wait: Duration::from_secs(2),
        });
        let client = SubsonicClient::with_timeouts(
            server.account(),
            Duration::from_millis(300),
            Duration::from_millis(300),
        )
        .unwrap();
        let err = client.ping().unwrap_err();
        assert!(matches!(err, NavidromeError::Unreachable(_)), "{err:?}");
        assert!(err.is_offline());
    }

    #[test]
    fn slow_first_byte_within_the_timeout_succeeds() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        server.set_fault(Fault::Stall {
            method: "ping",
            after_bytes: 0,
            wait: Duration::from_millis(400),
        });
        let client = SubsonicClient::with_timeouts(
            server.account(),
            Duration::from_secs(2),
            Duration::from_secs(5),
        )
        .unwrap();
        client.ping().unwrap();
    }

    #[test]
    fn html_maps_to_not_subsonic() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        server.set_fault(Fault::Html);
        let err = client(&server).ping().unwrap_err();
        assert_eq!(
            err,
            NavidromeError::NotSubsonic("not a Subsonic server".into())
        );
        assert!(!err.is_offline());

        server.set_fault(Fault::Status(404));
        assert_eq!(
            client(&server).ping().unwrap_err(),
            NavidromeError::NotSubsonic("HTTP 404".into())
        );
        server.set_fault(Fault::Status(500));
        assert_eq!(
            client(&server).ping().unwrap_err(),
            NavidromeError::NotSubsonic("HTTP 500".into())
        );
    }

    #[test]
    fn proxy_gateway_errors_map_to_unreachable() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        for status in [502, 503, 504] {
            server.set_fault(Fault::Status(status));
            let err = client(&server).ping().unwrap_err();
            assert_eq!(err, NavidromeError::Unreachable(format!("HTTP {status}")));
            assert!(err.is_offline());
            assert_eq!(err.code(), "unreachable");
        }
    }

    #[test]
    fn gzip_responses_are_decoded() {
        let server = FakeServer::start(FakeLibrary::synthetic(3, 4));
        server.set_gzip(true);
        let client = client(&server);
        client.ping().unwrap();
        assert_eq!(client.search3_songs(0, PAGE).unwrap().len(), 12);
        assert!(server.gzipped_responses() >= 2);
    }

    /// An error chain like the one rustls produces under reqwest.
    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl StdError for Layer {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            self.1.as_deref().map(|l| l as &(dyn StdError + 'static))
        }
    }

    #[test]
    fn certificate_errors_map_to_tls() {
        let err = Layer(
            "error sending request",
            Some(Box::new(Layer(
                "client error (Connect)",
                Some(Box::new(Layer(
                    "invalid peer certificate: UnknownIssuer",
                    None,
                ))),
            ))),
        );
        let mapped = classify(&err);
        assert!(matches!(mapped, NavidromeError::Tls(_)), "{mapped:?}");
        assert!(mapped.to_string().contains("UnknownIssuer"));
        assert!(!mapped.is_offline());
        assert_eq!(mapped.code(), "tls");

        let plain = Layer(
            "error sending request",
            Some(Box::new(Layer("tcp connect error", None))),
        );
        assert!(classify(&plain).is_offline());
    }

    /// Neither the password, the token, the salt nor a query parameter appear in the error.
    fn assert_clean(err: &NavidromeError, server: &FakeServer) {
        let texts = [
            err.to_string(),
            format!("{err:?}"),
            format!("{:?}", NavidromeFailure::from(err)),
            serde_json::to_string(&NavidromeFailure::from(err)).unwrap(),
        ];
        let mut secrets: Vec<String> = vec![FAKE_PASSWORD.into(), "t=".into(), "s=".into()];
        for request in server.requests() {
            secrets.extend(param(&request.params, "t").map(str::to_owned));
            secrets.extend(param(&request.params, "s").map(str::to_owned));
        }
        for text in &texts {
            for secret in &secrets {
                assert!(
                    !text.contains(secret.as_str()),
                    "{text:?} contains {secret:?}"
                );
            }
        }
    }

    #[test]
    fn errors_never_contain_the_token() {
        let faults = [
            Fault::WrongPassword,
            Fault::Status(401),
            Fault::Status(404),
            Fault::Status(500),
            Fault::Status(502),
            Fault::Status(503),
            Fault::Status(504),
            Fault::Html,
            Fault::Refuse,
            Fault::Stall {
                method: "ping",
                after_bytes: 0,
                wait: Duration::from_secs(1),
            },
            Fault::Stall {
                method: "ping",
                after_bytes: 10,
                wait: Duration::from_secs(1),
            },
            Fault::CutOnce {
                method: "ping",
                after_bytes: 5,
            },
            Fault::CutOnce {
                method: "getCoverArt",
                after_bytes: 5,
            },
            Fault::Stall {
                method: "getCoverArt",
                after_bytes: 0,
                wait: Duration::from_secs(1),
            },
            Fault::Stall {
                method: "getCoverArt",
                after_bytes: 10,
                wait: Duration::from_secs(1),
            },
            Fault::Code(0),
            Fault::Code(40),
            Fault::Code(41),
            Fault::Code(50),
            Fault::Code(70),
            Fault::StatusWithCode {
                status: 401,
                code: 40,
            },
        ];
        for fault in faults {
            let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
            server.set_fault(fault.clone());
            let client = SubsonicClient::with_timeouts(
                server.account(),
                Duration::from_millis(300),
                Duration::from_millis(300),
            )
            .unwrap();
            // The cover call reads its body differently (io::Read over the response).
            let errors: Vec<NavidromeError> = [
                client.ping().err(),
                client.cover_art("al-0_cover", 64).err(),
            ]
            .into_iter()
            .flatten()
            .collect();
            assert!(!errors.is_empty(), "{fault:?} produced no error");
            for err in &errors {
                assert_clean(err, &server);
            }
        }

        // Corrupt gzip and a refused redirect, on both call paths.
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        server.set_corrupt_gzip(true);
        assert_clean(&client(&server).ping().unwrap_err(), &server);
        server.set_corrupt_gzip(false);
        server.set_redirect(Some(Redirect::To(
            302,
            format!("http://localhost:{}", dead_port()),
        )));
        assert_clean(&client(&server).ping().unwrap_err(), &server);
        assert_clean(
            &client(&server).cover_art("al-0_cover", 64).unwrap_err(),
            &server,
        );
    }

    #[test]
    fn cover_size_cap() {
        let cap = MAX_COVER_BYTES as usize;
        let lib = FakeLibrary::synthetic(1, 1)
            .with_cover("exact", vec![7u8; cap])
            .with_cover("over", vec![7u8; cap + 1]);
        let server = FakeServer::start(lib);
        let client = client(&server);
        assert_eq!(client.cover_art("exact", 600).unwrap().len(), cap);
        assert_eq!(
            client.cover_art("over", 600).unwrap_err(),
            NavidromeError::NotSubsonic("the cover is too large".into())
        );
    }

    #[test]
    fn cover_cut_and_stall_map_to_unreachable() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let client = SubsonicClient::with_timeouts(
            server.account(),
            Duration::from_millis(500),
            Duration::from_millis(300),
        )
        .unwrap();
        server.set_fault(Fault::CutOnce {
            method: "getCoverArt",
            after_bytes: 5,
        });
        let err = client.cover_art("al-0_cover", 64).unwrap_err();
        assert!(err.is_offline(), "{err:?}");
        assert_clean(&err, &server);
        server.set_fault(Fault::Stall {
            method: "getCoverArt",
            after_bytes: 10,
            wait: Duration::from_secs(1),
        });
        let err = client.cover_art("al-0_cover", 64).unwrap_err();
        assert!(err.is_offline(), "{err:?}");
        assert_clean(&err, &server);
    }

    #[test]
    fn account_debug_hides_password() {
        let account = Account {
            url: "http://127.0.0.1:1".into(),
            user: FAKE_USER.into(),
            password: Secret::new(FAKE_PASSWORD),
        };
        let text = format!("{account:?}");
        assert!(!text.contains(FAKE_PASSWORD), "{text}");
        assert!(text.contains("***"));
    }

    #[test]
    fn search3_pages_all_songs() {
        let server = FakeServer::start(FakeLibrary::synthetic(120, 10));
        let client = client(&server);
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let page = client.search3_songs(offset, PAGE).unwrap();
            let got = page.len() as u32;
            all.extend(page);
            if got < PAGE {
                break;
            }
            offset += got;
        }
        assert_eq!(all.len(), 1200);
        let ids: HashSet<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 1200);
        let requests = server.requests();
        let offsets: Vec<&str> = requests
            .iter()
            .map(|r| param(&r.params, "songOffset").unwrap())
            .collect();
        assert_eq!(offsets, ["0", "500", "1000"]);
        for request in &requests {
            assert_eq!(request.method, "search3");
            assert_eq!(param(&request.params, "query"), Some(""));
            assert_eq!(param(&request.params, "artistCount"), Some("0"));
            assert_eq!(param(&request.params, "albumCount"), Some("0"));
            assert_eq!(param(&request.params, "songCount"), Some("500"));
        }
    }

    #[test]
    fn library_calls_read_the_fake_library() {
        let server = FakeServer::start(FakeLibrary::synthetic(3, 2));
        let client = client(&server);
        let before = client.indexes_last_modified().unwrap();
        assert!(before > 0);
        let albums = client.album_list2(0, PAGE).unwrap();
        assert_eq!(albums.len(), 3);
        let (album, songs) = client.album(&albums[0].id).unwrap();
        assert_eq!(album.id, albums[0].id);
        assert_eq!(songs.len(), 2);
        assert_eq!(client.album("nope").unwrap_err(), NavidromeError::NotFound);
        let playlists = client.playlists().unwrap();
        assert!(!playlists.is_empty());
        let playlist = client.playlist(&playlists[0].id).unwrap();
        assert_eq!(playlist.summary.id, playlists[0].id);
        assert_eq!(playlist.entry.len() as u32, playlists[0].song_count);

        server.edit(|lib| lib.albums.truncate(2));
        assert!(client.indexes_last_modified().unwrap() > before);
        assert_eq!(client.album_list2(0, PAGE).unwrap().len(), 2);

        let png = client
            .cover_art(albums[0].cover_art.as_deref().unwrap(), 32)
            .unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(
            client.cover_art("missing", 32).unwrap_err(),
            NavidromeError::NotFound
        );

        assert_eq!(client.search3_songs(0, PAGE).unwrap().len(), 6);
        server.set_fault(Fault::Search3Empty);
        assert!(client.search3_songs(0, PAGE).unwrap().is_empty());
    }

    #[test]
    fn starred2_requires_its_key() {
        let server = FakeServer::start(FakeLibrary::synthetic(2, 2));
        let client = client(&server);
        let expected = NavidromeError::NotSubsonic("unexpected starred2 response".into());
        server.set_fault(Fault::Payload {
            method: "getStarred2",
            payload: json!({}),
        });
        assert_eq!(client.starred2().unwrap_err(), expected);
        server.set_fault(Fault::Payload {
            method: "getStarred2",
            payload: json!({ "starred2": null }),
        });
        assert_eq!(client.starred2().unwrap_err(), expected);
        // Present but empty: nothing is starred.
        server.set_fault(Fault::Payload {
            method: "getStarred2",
            payload: json!({ "starred2": {} }),
        });
        assert_eq!(client.starred2().unwrap(), Starred2::default());
    }

    #[test]
    fn star_targets_use_the_right_param() {
        let server = FakeServer::start(FakeLibrary::synthetic(2, 2));
        let client = client(&server);
        client.star(&StarTarget::Song("tr-1".into()), true).unwrap();
        client
            .star(&StarTarget::Album("al-1".into()), true)
            .unwrap();
        client
            .star(&StarTarget::Artist("ar-0".into()), true)
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests[0].method, "star");
        assert_eq!(param(&requests[0].params, "id"), Some("tr-1"));
        assert_eq!(param(&requests[1].params, "albumId"), Some("al-1"));
        assert_eq!(param(&requests[2].params, "artistId"), Some("ar-0"));
        let (songs, albums, artists) = server.starred();
        assert!(songs.contains("tr-1") && albums.contains("al-1") && artists.contains("ar-0"));
        let starred = client.starred2().unwrap();
        assert_eq!(
            (
                starred.song.len(),
                starred.album.len(),
                starred.artist.len()
            ),
            (1, 1, 1)
        );

        client
            .star(&StarTarget::Song("tr-1".into()), false)
            .unwrap();
        assert_eq!(server.requests()[4].method, "unstar");
        assert!(server.starred().0.is_empty());

        client.scrobble("tr-1", 1_700_000_000_000, true).unwrap();
        client.scrobble("tr-2", 1_700_000_000_001, false).unwrap();
        assert_eq!(
            server.scrobbles(),
            [
                ("tr-1".to_owned(), true, Some(1_700_000_000_000)),
                ("tr-2".to_owned(), false, Some(1_700_000_000_001)),
            ]
        );
    }

    #[test]
    fn stream_url_quality_params() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let client = client(&server);
        let pairs = |url: &SecretUrl| -> Vec<(String, String)> {
            Url::parse(url.expose())
                .unwrap()
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        };

        let original = client.stream_url("tr-0", StreamQuality::Original, None);
        let p = pairs(&original);
        assert!(original.expose().contains("/rest/stream?"));
        assert_eq!(param(&p, "id"), Some("tr-0"));
        assert_eq!(param(&p, "format"), Some("raw"));
        assert_eq!(param(&p, "maxBitRate"), None);
        assert_eq!(param(&p, "estimateContentLength"), Some("false"));
        assert_eq!(param(&p, "timeOffset"), None);
        assert_eq!(
            param(&p, "t").map(str::to_owned),
            Some(token(FAKE_PASSWORD, param(&p, "s").unwrap()))
        );
        assert_eq!(format!("{original}"), "<redacted url>");
        assert_eq!(format!("{original:?}"), "<redacted url>");

        for (quality, rate) in [
            (StreamQuality::Kbps320, "320"),
            (StreamQuality::Kbps192, "192"),
            (StreamQuality::Kbps128, "128"),
        ] {
            let p = pairs(&client.stream_url("tr-0", quality, Some(42)));
            assert_eq!(param(&p, "format"), Some("mp3"));
            assert_eq!(param(&p, "maxBitRate"), Some(rate));
            assert_eq!(param(&p, "timeOffset"), Some("42"));
        }

        assert_eq!(StreamQuality::default(), StreamQuality::Original);
        assert_eq!(StreamQuality::Original.max_bitrate(), None);
        assert_eq!(StreamQuality::Kbps192.max_bitrate(), Some(192));
        assert_eq!(
            serde_json::to_string(&StreamQuality::Original).unwrap(),
            "\"original\""
        );
        assert_eq!(
            serde_json::to_string(&StreamQuality::Kbps320).unwrap(),
            "\"320\""
        );
        assert_eq!(
            serde_json::from_str::<StreamQuality>("\"128\"").unwrap(),
            StreamQuality::Kbps128
        );
    }

    #[test]
    fn stream_http_serves_ranges_without_a_total_timeout() {
        let lib = FakeLibrary::synthetic(1, 1)
            .with_audio("tr-0", (0..=255u8).cycle().take(10_000).collect());
        let server = FakeServer::start(lib);
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 0,
            wait: Duration::from_millis(500),
        });
        let client = SubsonicClient::with_timeouts(
            server.account(),
            Duration::from_secs(2),
            Duration::from_millis(100),
        )
        .unwrap();
        let url = client.stream_url("tr-0", StreamQuality::Original, None);
        // Slower than the API timeout to the first byte: the stream client still waits.
        let response = client
            .stream_http()
            .get(url.expose())
            .header("Range", "bytes=100-")
            .send()
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let body = response.bytes().unwrap();
        assert_eq!(body.len(), 9_900);
        assert_eq!(body[0], 100);
        assert_eq!(server.requests()[0].range.as_deref(), Some("bytes=100-"));
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "subsonic",
            name,
        ]
        .iter()
        .collect();
        std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn parse<T: DeserializeOwned + Default>(name: &str, key: &str) -> T {
        let mut response = envelope(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(response.get(key).is_some(), "{name} has no {key}");
        field(&mut response, key).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[test]
    fn fixtures_parse() {
        let ping = envelope(&fixture("ping.json")).unwrap();
        assert_eq!(ping["status"], "ok");
        assert_eq!(
            envelope(&fixture("error_40.json")).unwrap_err(),
            NavidromeError::WrongCredentials
        );
        assert_eq!(
            envelope(&fixture("error_41.json")).unwrap_err(),
            NavidromeError::TokenAuthUnsupported
        );

        let indexes: Indexes = parse("getIndexes.json", "indexes");
        assert!(indexes.last_modified > 0);
        let albums: AlbumList2 = parse("getAlbumList2.json", "albumList2");
        assert!(!albums.album.is_empty());
        assert!(albums
            .album
            .iter()
            .all(|a| !a.id.is_empty() && !a.name.is_empty()));
        let album: AlbumWithSongs = parse("getAlbum.json", "album");
        assert!(!album.album.id.is_empty());
        assert!(!album.song.is_empty());
        assert!(album.song[0].replay_gain.is_some());
        assert!(!album.song[0].album_artists.is_empty());
        let search: SearchResult3 = parse("search3.json", "searchResult3");
        assert!(!search.song.is_empty());
        assert!(search.song.iter().all(|s| s.duration.is_some()));
        let starred: Starred2 = parse("getStarred2.json", "starred2");
        assert!(
            !starred.song.is_empty() && !starred.album.is_empty() && !starred.artist.is_empty()
        );
        assert!(starred.song.iter().all(|s| s.starred.is_some()));
        let playlists: Playlists = parse("getPlaylists.json", "playlists");
        assert!(!playlists.playlist.is_empty());
        let playlist: PlaylistWithSongs = parse("getPlaylist.json", "playlist");
        assert!(!playlist.summary.id.is_empty());
        assert_eq!(playlist.entry.len() as u32, playlist.summary.song_count);
    }

    #[test]
    fn failure_serializes_code_and_detail() {
        let failure = NavidromeFailure::from(&NavidromeError::Unreachable("HTTP 502".into()));
        assert_eq!(
            serde_json::to_value(&failure).unwrap(),
            serde_json::json!({ "code": "unreachable", "detail": "HTTP 502" })
        );
        let failure = NavidromeFailure::from(&NavidromeError::WrongCredentials);
        assert_eq!(failure.detail, None);
    }

    /// The Subsonic methods [`smoke`] may call: reads only, never a star, scrobble or edit.
    const SMOKE_METHODS: &[&str] = &[
        "ping",
        "getAlbumList2",
        "search3",
        "getCoverArt",
        "stream",
        "getPlaylists",
        "getPlaylist",
    ];

    /// The most playlists [`smoke`] opens with `getPlaylist`.
    const SMOKE_PLAYLISTS: usize = 25;

    /// Unwraps a smoke step; a failure panics with the step and the error code only, never the
    /// error text (it can quote the server).
    fn step<T>(name: &str, result: Result<T, NavidromeError>) -> T {
        result.unwrap_or_else(|err| panic!("{name} failed: {}", err.code()))
    }

    fn ms(started: std::time::Instant) -> u128 {
        started.elapsed().as_millis()
    }

    /// The codec the first bytes of a stream look like.
    fn sniff(bytes: &[u8]) -> &'static str {
        match bytes {
            [b'I', b'D', b'3', ..] => "mp3 (ID3)",
            [0xFF, b, ..] if b & 0xE0 == 0xE0 => "mp3",
            [b'f', b'L', b'a', b'C', ..] => "flac",
            [b'O', b'g', b'g', b'S', ..] => "ogg",
            [b'R', b'I', b'F', b'F', ..] => "wav",
            [_, _, _, _, b'f', b't', b'y', b'p', ..] => "mp4",
            [] => "empty",
            _ => "unknown",
        }
    }

    /// A file suffix as a format name; anything that does not look like one is not printed.
    fn format_of(suffix: Option<&str>) -> String {
        match suffix {
            Some(s)
                if !s.is_empty()
                    && s.len() <= 8
                    && s.chars().all(|c| c.is_ascii_alphanumeric()) =>
            {
                s.to_ascii_lowercase()
            }
            Some(_) => "other".into(),
            None => "none".into(),
        }
    }

    /// A response header as printable text: media types and numbers only.
    fn header(response: &reqwest::blocking::Response, name: reqwest::header::HeaderName) -> String {
        match response.headers().get(name).and_then(|v| v.to_str().ok()) {
            Some(v)
                if v.len() <= 64
                    && v.chars().all(|c| {
                        c.is_ascii_alphanumeric()
                            || matches!(c, '/' | '-' | '+' | '.' | ';' | '=' | ' ')
                    }) =>
            {
                v.to_owned()
            }
            Some(_) => "unprintable".into(),
            None => "none".into(),
        }
    }

    /// Opens a stream and reads at most `limit` bytes of it. Returns the response (headers
    /// only by then), the bytes and the time to the headers.
    fn stream_head(
        client: &SubsonicClient,
        id: &str,
        quality: StreamQuality,
        limit: u64,
    ) -> Result<(reqwest::blocking::Response, Vec<u8>, u128), NavidromeError> {
        let url = client.stream_url(id, quality, None);
        let started = std::time::Instant::now();
        let response = client
            .stream_http()
            .get(url.expose())
            .send()
            .map_err(classify_reqwest)?;
        let headers_ms = ms(started);
        let mut bytes = Vec::new();
        if response.status().is_success() {
            // Reading through `&mut` keeps the response, so its headers can still be printed.
            let mut body = response;
            (&mut body)
                .take(limit)
                .read_to_end(&mut bytes)
                .map_err(classify_io)?;
            return Ok((body, bytes, headers_ms));
        }
        Ok((response, bytes, headers_ms))
    }

    /// The read-only checks of [`live_server_smoke`], as printable lines: counts, timings,
    /// formats and status codes only, never a name, id, URL or credential.
    fn smoke(account: Account) -> Vec<String> {
        let client = SubsonicClient::new(account).unwrap();
        let mut out = Vec::new();

        let started = std::time::Instant::now();
        let info = step("ping", client.ping());
        out.push(format!(
            "ping: {} ms, api {}, openSubsonic {}",
            ms(started),
            if info.api_version.len() <= 16
                && info
                    .api_version
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.')
            {
                info.api_version.as_str()
            } else {
                "other"
            },
            info.open_subsonic
        ));

        let started = std::time::Instant::now();
        let five = step("getAlbumList2 (5)", client.album_list2(0, 5));
        out.push(format!(
            "getAlbumList2 page of 5: {} albums in {} ms",
            five.len(),
            ms(started)
        ));
        let started = std::time::Instant::now();
        let page = step("getAlbumList2 (500)", client.album_list2(0, PAGE));
        let no_artist_id = page
            .iter()
            .filter(|a| a.artist_id.as_deref().is_none_or(str::is_empty))
            .count();
        out.push(format!(
            "getAlbumList2 page of {PAGE}: {} albums in {} ms; without artistId: {no_artist_id}",
            page.len(),
            ms(started)
        ));

        let started = std::time::Instant::now();
        let songs = step("search3", client.search3_songs(0, PAGE));
        let count_missing =
            |missing: fn(&Child) -> bool| songs.iter().filter(|s| missing(s)).count();
        out.push(format!(
            "search3 page of {PAGE}: {} songs in {} ms; without albumId: {}, without artistId: {}, \
             without duration: {}",
            songs.len(),
            ms(started),
            count_missing(|s| s.album_id.as_deref().is_none_or(str::is_empty)),
            count_missing(|s| s.artist_id.as_deref().is_none_or(str::is_empty)),
            count_missing(|s| s.duration.is_none()),
        ));
        let mut formats: Vec<(String, usize)> = Vec::new();
        for song in &songs {
            let format = format_of(song.suffix.as_deref());
            match formats.iter_mut().find(|(known, _)| *known == format) {
                Some((_, n)) => *n += 1,
                None => formats.push((format, 1)),
            }
        }
        formats.sort();
        let formats: Vec<String> = formats.iter().map(|(f, n)| format!("{f} {n}")).collect();
        out.push(format!("formats in that page: {}", formats.join(", ")));

        match page.iter().chain(&five).find_map(|a| a.cover_art.clone()) {
            Some(cover) => {
                let started = std::time::Instant::now();
                let bytes = step("getCoverArt", client.cover_art(&cover, 96));
                out.push(format!(
                    "getCoverArt at 96: {} bytes in {} ms",
                    bytes.len(),
                    ms(started)
                ));
            }
            None => out.push("getCoverArt: no album with a cover in the first page".into()),
        }

        match songs.first() {
            Some(song) => {
                let started = std::time::Instant::now();
                let (response, bytes, headers_ms) = step(
                    "stream (original)",
                    stream_head(&client, &song.id, StreamQuality::Original, 64 * 1024),
                );
                out.push(format!(
                    "stream original ({}): status {}, headers in {headers_ms} ms, {} bytes in {} ms, \
                     content-type {}, content-length {}, accept-ranges {}, looks like {}",
                    format_of(song.suffix.as_deref()),
                    response.status().as_u16(),
                    bytes.len(),
                    ms(started),
                    header(&response, reqwest::header::CONTENT_TYPE),
                    if response.headers().contains_key(reqwest::header::CONTENT_LENGTH) {
                        "set"
                    } else {
                        "none"
                    },
                    header(&response, reqwest::header::ACCEPT_RANGES),
                    sniff(&bytes),
                ));
            }
            None => out.push("stream: no song in the first search3 page".into()),
        }

        // Task 17 labels a stream at a bitrate cap as MP3 unless the file is an MP3 at or below
        // the cap: this shows whether the server really transcodes a non-MP3 file.
        let other = songs.iter().find(|s| {
            s.suffix
                .as_deref()
                .is_some_and(|suffix| !suffix.eq_ignore_ascii_case("mp3"))
        });
        match other {
            Some(song) => {
                let (response, bytes, headers_ms) = step(
                    "stream (320)",
                    stream_head(&client, &song.id, StreamQuality::Kbps320, 16 * 1024),
                );
                out.push(format!(
                    "stream format=mp3&maxBitRate=320 of a {} file: status {}, headers in \
                     {headers_ms} ms, content-type {}, content-length {}, looks like {}",
                    format_of(song.suffix.as_deref()),
                    response.status().as_u16(),
                    header(&response, reqwest::header::CONTENT_TYPE),
                    header(&response, reqwest::header::CONTENT_LENGTH),
                    sniff(&bytes),
                ));
            }
            None => out.push("transcode check: no non-MP3 song in the first search3 page".into()),
        }

        // A playlist whose listed songCount or changed differ from its getPlaylist answer is
        // fetched again every cycle (Task 12's change check).
        let started = std::time::Instant::now();
        let listed = step("getPlaylists", client.playlists());
        out.push(format!(
            "getPlaylists: {} playlists in {} ms",
            listed.len(),
            ms(started)
        ));
        let (mut checked, mut count_differs, mut changed_differs, mut changed_missing) =
            (0, 0, 0, 0);
        let mut entries_differ = 0;
        let started = std::time::Instant::now();
        for summary in listed.iter().take(SMOKE_PLAYLISTS) {
            let full = step("getPlaylist", client.playlist(&summary.id));
            checked += 1;
            count_differs += usize::from(summary.song_count != full.summary.song_count);
            changed_differs += usize::from(summary.changed != full.summary.changed);
            changed_missing += usize::from(summary.changed.is_none());
            entries_differ += usize::from(full.entry.len() as u32 != full.summary.song_count);
        }
        out.push(format!(
            "getPlaylist of {checked} in {} ms: songCount differs from the listing in \
             {count_differs}, changed differs in {changed_differs}, changed missing in the listing \
             in {changed_missing}, entries differ from songCount in {entries_differ}",
            ms(started)
        ));
        out
    }

    /// Read-only checks against a real server, for the owner to run by hand. The server comes
    /// from `PRISM_LIVE_SUBSONIC_URL`, `PRISM_LIVE_SUBSONIC_USER` and
    /// `PRISM_LIVE_SUBSONIC_PASSWORD` only; without them the test passes with a message. It
    /// prints counts, timings, formats and status codes, never names, ids, URLs or credentials.
    #[test]
    #[ignore = "talks to the server in PRISM_LIVE_SUBSONIC_URL; run by hand"]
    fn live_server_smoke() {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let (Some(url), Some(user), Some(password)) = (
            var("PRISM_LIVE_SUBSONIC_URL"),
            var("PRISM_LIVE_SUBSONIC_USER"),
            var("PRISM_LIVE_SUBSONIC_PASSWORD"),
        ) else {
            println!(
                "live_server_smoke skipped: set PRISM_LIVE_SUBSONIC_URL, PRISM_LIVE_SUBSONIC_USER \
                 and PRISM_LIVE_SUBSONIC_PASSWORD"
            );
            return;
        };
        let url = normalize_url(&url)
            .unwrap_or_else(|err| panic!("PRISM_LIVE_SUBSONIC_URL is not usable: {}", err.code()));
        let account = Account {
            url,
            user: user.trim().to_owned(),
            password: Secret::new(password),
        };
        for line in smoke(account) {
            println!("{line}");
        }
    }

    /// The smoke test itself, against the fake server: it only reads, and nothing it prints
    /// names the server, the account or the library.
    #[test]
    fn smoke_reads_only_and_prints_no_names() {
        let lib = FakeLibrary::synthetic(3, 4)
            .with_audio("tr-0", [b"ID3".as_slice(), &[0u8; 70_000]].concat())
            .with_wav("tr-5", 2);
        let server = FakeServer::start(lib);
        let lines = smoke(server.account());
        let text = lines.join("\n");
        println!("{text}");
        let lower = text.to_lowercase();
        for secret in [
            "demo",
            "127.0.0.1",
            "http",
            "tr-",
            "al-",
            "pl-",
            FAKE_PASSWORD,
        ] {
            assert!(!lower.contains(secret), "{secret:?} in:\n{text}");
        }
        assert!(text.contains("without artistId: 0"), "{text}");
        assert!(text.contains("65536 bytes"), "{text}");
        assert!(text.contains("of a wav file: status 200"), "{text}");
        assert!(text.contains("getPlaylist of 2 in"), "{text}");
        assert!(
            text.contains("songCount differs from the listing in 0"),
            "{text}"
        );
        let methods: HashSet<String> = server.requests().into_iter().map(|r| r.method).collect();
        assert!(
            methods.iter().all(|m| SMOKE_METHODS.contains(&m.as_str())),
            "{methods:?}"
        );
        assert!(server.starred().0.is_empty() && server.scrobbles().is_empty());
    }
}
