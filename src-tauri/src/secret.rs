//! Values that must never reach logs, error texts, events or `Debug` output: passwords, and
//! URLs that carry a Subsonic token and salt.

use std::fmt;

/// A password (or any other secret string). `Debug` and `Display` print `***`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The secret itself, for the few places that must send or store it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Secret {
        Secret(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Secret {
        Secret(value.to_owned())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A URL with credentials in its query (a Subsonic stream URL). `Debug` and `Display` print
/// `<redacted url>`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretUrl(String);

impl SecretUrl {
    pub fn new(value: impl Into<String>) -> SecretUrl {
        SecretUrl(value.into())
    }

    /// The full URL, for the HTTP request itself only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted url>")
    }
}

impl fmt::Display for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted url>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_prints_redacted() {
        let secret = Secret::new("demo-pass");
        assert_eq!(format!("{secret}"), "***");
        assert_eq!(format!("{secret:?}"), "***");
        assert_eq!(secret.expose(), "demo-pass");
    }

    #[test]
    fn secret_url_prints_redacted() {
        let url = SecretUrl::new("http://127.0.0.1:1/rest/stream?t=abc&s=def");
        assert_eq!(format!("{url}"), "<redacted url>");
        assert_eq!(format!("{url:?}"), "<redacted url>");
        assert!(url.expose().contains("t=abc"));
    }
}
