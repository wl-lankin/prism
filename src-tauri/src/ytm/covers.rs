//! YouTube Music artwork for the resize gate (`yt-` covers). The id is a hash of the URL the
//! page reported last; the URL itself stays in memory (the last [`MAX_ARTWORK`]), so after a
//! restart only covers already on disk are served. Fetches go to the artwork hosts only, over
//! https, never into a private network, capped like station logos.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex};

use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use reqwest::Url;
use sha1::{Digest, Sha1};

use crate::radio::covers::{
    decodes, refused_as_private, PrivateAddress, PublicResolver, CONNECT_TIMEOUT,
    FAVICON_FETCH_LIMIT, FAVICON_TIMEOUT, MAX_FAVICON, MAX_REDIRECTS,
};
use crate::radio::directory::USER_AGENT;
use crate::thumbs::{RemoteCovers, ResizeError, ResizeGate};

use super::lock;

pub const YTM_COVER_PREFIX: &str = "yt-";
/// Artwork URLs remembered at most: room for a full queue's thumbnails (200) without pushing
/// out the artwork of what plays.
pub const MAX_ARTWORK: usize = 512;

/// The hosts YouTube Music serves artwork from.
const HOST_SUFFIXES: [&str; 2] = [".googleusercontent.com", ".ggpht.com"];
const HOSTS: [&str; 1] = ["i.ytimg.com"];

/// True for an https URL on an artwork host, on the default port, without user info.
pub fn artwork_allowed(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && (HOSTS.contains(&host.as_str())
            || HOST_SUFFIXES
                .iter()
                .any(|suffix| host.len() > suffix.len() && host.ends_with(suffix)))
}

/// `yt-<first 20 hex of sha1(url)>`.
pub fn cover_id(url: &str) -> String {
    let digest = hex::encode(Sha1::digest(url.as_bytes()));
    format!("{YTM_COVER_PREFIX}{}", &digest[..20])
}

/// The 20 lowercase hex digits of an artwork cover id, `None` for anything else.
pub fn ytm_cover_raw(id: &str) -> Option<&str> {
    let raw = id.strip_prefix(YTM_COVER_PREFIX)?;
    let hex = raw.len() == 20
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    hex.then_some(raw)
}

#[derive(Default)]
struct Remembered {
    /// Raw id (without the prefix) to URL.
    urls: HashMap<String, String>,
    /// Oldest first.
    order: VecDeque<String>,
}

/// The artwork URLs behind the `yt-` ids, the last [`MAX_ARTWORK`] reported.
#[derive(Default)]
pub struct Artwork {
    inner: Mutex<Remembered>,
}

impl Artwork {
    /// Remembers an already validated URL and returns its cover id.
    pub fn remember(&self, url: &str) -> String {
        let id = cover_id(url);
        let raw = id[YTM_COVER_PREFIX.len()..].to_string();
        let mut inner = lock(&self.inner);
        if inner.urls.contains_key(&raw) {
            inner.order.retain(|known| known != &raw);
        } else {
            inner.urls.insert(raw.clone(), url.to_string());
        }
        inner.order.push_back(raw);
        while inner.order.len() > MAX_ARTWORK {
            if let Some(oldest) = inner.order.pop_front() {
                inner.urls.remove(&oldest);
            }
        }
        id
    }

    pub fn url_of(&self, raw: &str) -> Option<String> {
        lock(&self.inner).urls.get(raw).cloned()
    }
}

type Allow = Arc<dyn Fn(&Url) -> bool + Send + Sync>;

pub struct YtmCovers {
    artwork: Arc<Artwork>,
    client: Client,
    allow: Allow,
}

impl YtmCovers {
    pub fn new(artwork: Arc<Artwork>) -> anyhow::Result<YtmCovers> {
        YtmCovers::with_allow(artwork, Arc::new(artwork_allowed))
    }

    /// `allow` decides every URL, redirects included. Only the tests, whose fake host is on
    /// 127.0.0.1, pass anything but [`artwork_allowed`].
    fn with_allow(artwork: Arc<Artwork>, allow: Allow) -> anyhow::Result<YtmCovers> {
        let follow = Arc::clone(&allow);
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .dns_resolver(Arc::new(PublicResolver))
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() > MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if !follow(attempt.url()) {
                    attempt.error(PrivateAddress)
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(YtmCovers {
            artwork,
            client,
            allow,
        })
    }
}

impl RemoteCovers for YtmCovers {
    fn fetch(&self, raw: &str, _size: u32) -> Result<Vec<u8>, ResizeError> {
        let url = self
            .artwork
            .url_of(raw)
            .and_then(|url| Url::parse(&url).ok())
            .ok_or(ResizeError::Unknown)?;
        if !(self.allow)(&url) {
            return Err(ResizeError::Unknown);
        }
        #[cfg(test)]
        crate::radio::testserver::assert_loopback(url.as_str());
        let response = match self
            .client
            .get(url)
            .timeout(FAVICON_TIMEOUT)
            .header(reqwest::header::ACCEPT, "image/*")
            .send()
        {
            Ok(response) if response.status().is_success() => response,
            Ok(_) => return Err(ResizeError::Unknown),
            Err(err) if err.is_redirect() || err.is_status() || refused_as_private(&err) => {
                return Err(ResizeError::Unknown)
            }
            Err(_) => return Err(ResizeError::Unreachable),
        };
        if response
            .content_length()
            .is_some_and(|len| len > MAX_FAVICON)
        {
            return Err(ResizeError::Unknown);
        }
        let mut bytes = Vec::new();
        if response
            .take(MAX_FAVICON + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Err(ResizeError::Unreachable);
        }
        if bytes.len() as u64 > MAX_FAVICON {
            return Err(ResizeError::Unknown);
        }
        if !decodes(&bytes) {
            return Err(ResizeError::Undecodable("not an image".into()));
        }
        Ok(bytes)
    }

    fn fetch_limit(&self) -> usize {
        FAVICON_FETCH_LIMIT
    }
}

/// Plugs the artwork into the gate.
pub fn attach(gate: &ResizeGate, artwork: Arc<Artwork>) -> anyhow::Result<()> {
    gate.set_remote_for(YTM_COVER_PREFIX, Arc::new(YtmCovers::new(artwork)?));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::testserver::{FakeWeb, Page};
    use image::{Rgba, RgbaImage};
    use std::io::Cursor;

    fn png() -> Vec<u8> {
        let image = RgbaImage::from_pixel(16, 16, Rgba([0xff, 0xc9, 0x4d, 0xff]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn parsed(url: &str) -> Url {
        Url::parse(url).unwrap()
    }

    #[test]
    fn artwork_gets_a_stable_cover_id() {
        let url = "https://lh3.googleusercontent.com/abc=w544-h544";
        let id = cover_id(url);
        assert_eq!(id, cover_id(url));
        assert!(id.starts_with(YTM_COVER_PREFIX));
        assert_eq!(id.len(), YTM_COVER_PREFIX.len() + 20);
        assert_eq!(ytm_cover_raw(&id), Some(&id[3..]));
        assert_ne!(
            id,
            cover_id("https://lh3.googleusercontent.com/abd=w544-h544")
        );
        for bad in [
            "yt-",
            "yt-XYZ",
            "rb-0123456789abcdef0123",
            "yt-0123456789ABCDEF0123",
        ] {
            assert_eq!(ytm_cover_raw(bad), None, "{bad}");
        }

        let artwork = Artwork::default();
        assert_eq!(artwork.remember(url), id);
        assert_eq!(artwork.url_of(&id[3..]).as_deref(), Some(url));
        // The oldest is forgotten past the cap; one reported again moves to the end.
        for n in 0..MAX_ARTWORK - 1 {
            artwork.remember(&format!("https://i.ytimg.com/{n}.jpg"));
        }
        artwork.remember(url);
        artwork.remember("https://i.ytimg.com/new.jpg");
        assert_eq!(artwork.url_of(&id[3..]).as_deref(), Some(url));
        let first = cover_id("https://i.ytimg.com/0.jpg");
        assert_eq!(artwork.url_of(&first[3..]), None);
        assert_eq!(lock(&artwork.inner).urls.len(), MAX_ARTWORK);
        assert_eq!(lock(&artwork.inner).order.len(), MAX_ARTWORK);
    }

    #[test]
    fn covers_only_from_allowed_hosts() {
        for good in [
            "https://lh3.googleusercontent.com/a",
            "https://yt3.ggpht.com/a",
            "https://i.ytimg.com/vi/x/hq.jpg",
            "https://LH3.GOOGLEUSERCONTENT.COM/a",
        ] {
            assert!(artwork_allowed(&parsed(good)), "{good}");
        }
        for bad in [
            "http://lh3.googleusercontent.com/a",
            "https://googleusercontent.com/a",
            "https://.ggpht.com/a",
            "https://evilgoogleusercontent.com/a",
            "https://googleusercontent.com.evil.example/a",
            "https://x.ytimg.com/a",
            "https://i.ytimg.com:444/a",
            "https://user:pw@i.ytimg.com/a",
            "https://127.0.0.1/a",
            "https://[::1]/a",
            "ftp://i.ytimg.com/a",
            "file:///etc/passwd",
        ] {
            assert!(!artwork_allowed(&parsed(bad)), "{bad}");
        }

        // A remembered URL off the allowed hosts is never fetched (it would panic in
        // `assert_loopback` if it were).
        let artwork = Arc::new(Artwork::default());
        let id = artwork.remember("https://evil.example/a.png");
        let covers = YtmCovers::new(Arc::clone(&artwork)).unwrap();
        assert_eq!(covers.fetch(&id[3..], 300), Err(ResizeError::Unknown));
        assert_eq!(
            covers.fetch("0123456789abcdef0123", 300),
            Err(ResizeError::Unknown)
        );

        // Through the client: an image comes back; a redirect off the allowed origin, a
        // page that is not an image and one too large are refused.
        let web = FakeWeb::start();
        let origin = parsed(&web.url("/")).origin();
        let covers = YtmCovers::with_allow(
            Arc::clone(&artwork),
            Arc::new(move |url: &Url| url.origin() == origin),
        )
        .unwrap();
        web.set("/art.png", Page::ok("image/png", png()));
        let id = artwork.remember(&web.url("/art.png"));
        assert_eq!(covers.fetch(&id[3..], 300), Ok(png()));

        let other = FakeWeb::start();
        other.set("/art.png", Page::ok("image/png", png()));
        web.set("/away", Page::redirect(&other.url("/art.png")));
        let id = artwork.remember(&web.url("/away"));
        assert_eq!(covers.fetch(&id[3..], 300), Err(ResizeError::Unknown));
        assert_eq!(other.hits("/art.png"), 0);

        web.set("/page", Page::ok("text/html", "<html></html>"));
        let id = artwork.remember(&web.url("/page"));
        assert!(matches!(
            covers.fetch(&id[3..], 300),
            Err(ResizeError::Undecodable(_))
        ));

        let huge = vec![0u8; MAX_FAVICON as usize + 1];
        web.set("/huge", Page::ok("image/png", huge.clone()));
        let id = artwork.remember(&web.url("/huge"));
        assert_eq!(covers.fetch(&id[3..], 300), Err(ResizeError::Unknown));
        web.set(
            "/huge-unsized",
            Page::ok("image/png", huge).without_length(),
        );
        let id = artwork.remember(&web.url("/huge-unsized"));
        assert_eq!(covers.fetch(&id[3..], 300), Err(ResizeError::Unknown));

        web.set("/gone", Page::status(404));
        let id = artwork.remember(&web.url("/gone"));
        assert_eq!(covers.fetch(&id[3..], 300), Err(ResizeError::Unknown));
    }
}
