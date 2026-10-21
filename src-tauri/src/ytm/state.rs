//! What Prism knows about YouTube Music's player, and the commands it can send into it.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum YtmStatus {
    /// Nothing loaded (or the view does not exist yet).
    #[default]
    Idle,
    Playing,
    Paused,
    /// The page's player bar or buttons are missing: Prism cannot control it.
    Unavailable,
}

/// Payload of `ytm://state`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtmState {
    pub status: YtmStatus,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// `yt-<sha1(url) 20 hex>`, served by the cover gate; `None` without artwork.
    pub cover_id: Option<String>,
    pub position_ms: u64,
    /// 0 while unknown.
    pub duration_ms: u64,
    /// The page's next and previous buttons can be pressed.
    pub can_next: bool,
    pub can_prev: bool,
}

impl YtmState {
    pub fn is_playing(&self) -> bool {
        self.status == YtmStatus::Playing
    }
}

/// A command for YouTube Music's page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum YtmAction {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    /// Position in ms.
    Seek(u64),
    /// 0..=1.
    Volume(f32),
}

/// Sends commands into YouTube Music's page. Called from the player's event thread and
/// from commands, so it must not block and must not call back into the transport.
pub trait YtmControl: Send + Sync {
    fn send(&self, action: YtmAction);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_serializes_in_camel_case() {
        let state = YtmState {
            status: YtmStatus::Playing,
            title: "Song".into(),
            cover_id: Some("yt-0123456789abcdef0123".into()),
            position_ms: 1_000,
            duration_ms: 2_000,
            ..YtmState::default()
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "playing");
        assert_eq!(json["coverId"], "yt-0123456789abcdef0123");
        assert_eq!(json["positionMs"], 1_000);
        assert_eq!(json["durationMs"], 2_000);
        assert_eq!(
            serde_json::to_value(YtmStatus::Unavailable).unwrap(),
            "unavailable"
        );
        assert_eq!(YtmState::default().status, YtmStatus::Idle);
    }
}
