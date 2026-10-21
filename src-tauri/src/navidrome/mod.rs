//! Navidrome (Subsonic API) as a source.

pub mod client;
pub mod keychain;
pub mod sync;

#[cfg(test)]
pub(crate) mod testserver;
