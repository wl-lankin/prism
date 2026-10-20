//! The player service: queue, engine thread and the events it sends to the frontend.

pub mod dropped;
pub mod engine;
pub mod queue;

use std::sync::Arc;

use tauri::{AppHandle, Emitter};

pub use dropped::{with_dropped, DroppedFiles, DROPPED_PREFIX};
pub use engine::{
    default_opener, track_gain, EventSink, Opener, Player, PlayerError, PlayerEvent, PlayerOptions,
    PlayerState, PlayerStatus, Resolver, DEFAULT_VOLUME, PRELOAD_MS, RESTART_AFTER_MS,
};
pub use queue::Queue;

/// Payload: [`PlayerState`]. Sent on every change and every 250 ms while playing.
pub const EVENT_STATE: &str = "player://state";
/// Payload: [`PlayerError`]. Sent when a track cannot be opened or decoded.
pub const EVENT_ERROR: &str = "player://error";

/// Delivers every event to each of `sinks`, in order.
pub fn fan_out(sinks: Vec<EventSink>) -> EventSink {
    Arc::new(move |event| {
        for sink in &sinks {
            sink(event.clone());
        }
    })
}

/// Emits player events to every webview window. `TrackChanged` is internal: the frontend
/// sees the new track in the next `player://state`.
pub fn tauri_sink(app: AppHandle) -> EventSink {
    Arc::new(move |event| {
        let result = match &event {
            PlayerEvent::State(state) => app.emit(EVENT_STATE, state),
            PlayerEvent::Error(error) => app.emit(EVENT_ERROR, error),
            PlayerEvent::TrackChanged(_) => Ok(()),
        };
        if let Err(err) = result {
            eprintln!("emitting a player event failed: {err}");
        }
    })
}
