//! Internet radio: stations, their store and (in later tasks) the directory client, the
//! location, the live stream and the service that ties them together.

pub mod directory;
pub mod location;
pub mod model;
pub mod store;

#[cfg(test)]
pub(crate) mod testserver;
