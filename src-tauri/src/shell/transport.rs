//! One transport for both players: Prism's engine and YouTube Music. It knows which one is
//! active (the last one that started), sends transport commands there, and keeps only one of
//! them playing.
//!
//! Prism starts when its state goes from idle or paused to loading or playing (a play, a
//! jump, Play on a paused track, Play on a held station or an offline hold). Loading to
//! playing is the same start, and playing to loading is a track change, not a start.
//! YouTube Music starts on a real edge from not playing to playing in its reports.
//!
//! The page is untrusted and may flap, so a YouTube Music start that would pause Prism needs
//! a second report at least [`YTM_DEBOUNCE`] after the edge (the bridge's heartbeat brings
//! one within a second), and none counts within [`EXPECTED_PAUSE`] of Prism pausing the page
//! (those reports are the page reacting to that pause). A start found inside the window
//! still takes over once the window has passed, if the page still plays.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::player::{EventSink, PlayerEvent, PlayerHandle, PlayerStatus};
use crate::ytm::state::YtmState;
pub use crate::ytm::state::{YtmAction, YtmControl};

/// After Prism pauses YouTube Music, the page's reports this long do not count as a start.
pub const EXPECTED_PAUSE: Duration = Duration::from_secs(2);
/// A YouTube Music start pauses Prism only once a report this long after the edge confirms it.
pub const YTM_DEBOUNCE: Duration = Duration::from_millis(500);

/// The player transport commands go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Active {
    Prism,
    Ytm,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// What the transport needs from Prism's engine.
pub trait PrismControl: Send + Sync {
    fn toggle(&self) -> Result<(), String>;
    fn pause(&self) -> Result<(), String>;
    fn next(&self) -> Result<(), String>;
    fn prev(&self) -> Result<(), String>;
    fn seek(&self, ms: u64) -> Result<(), String>;
    fn set_volume(&self, volume: f32) -> Result<(), String>;
}

impl PrismControl for PlayerHandle {
    fn toggle(&self) -> Result<(), String> {
        PlayerHandle::toggle(self)
    }
    fn pause(&self) -> Result<(), String> {
        PlayerHandle::pause(self)
    }
    fn next(&self) -> Result<(), String> {
        PlayerHandle::next(self)
    }
    fn prev(&self) -> Result<(), String> {
        PlayerHandle::prev(self)
    }
    fn seek(&self, ms: u64) -> Result<(), String> {
        PlayerHandle::seek(self, ms)
    }
    fn set_volume(&self, volume: f32) -> Result<(), String> {
        PlayerHandle::set_volume(self, volume)
    }
}

struct Inner {
    active: Active,
    /// Prism's status from its last state event.
    prism: PlayerStatus,
    /// The transport paused Prism and Prism has not started since.
    prism_paused: bool,
    /// The page's last report said playing.
    ytm_playing: bool,
    /// When the transport paused the page; cleared when the page takes over.
    ytm_paused_at: Option<Instant>,
    /// The page's not-playing to playing edge that has not taken over yet.
    ytm_edge: Option<Instant>,
}

impl Inner {
    fn prism_busy(&self) -> bool {
        matches!(self.prism, PlayerStatus::Loading | PlayerStatus::Playing) && !self.prism_paused
    }

    /// Prism became active; true when the page has to be paused.
    fn prism_started(&mut self, now: Instant) -> bool {
        self.active = Active::Prism;
        self.prism_paused = false;
        self.ytm_edge = None;
        if self.ytm_playing && self.ytm_paused_at.is_none() {
            self.ytm_paused_at = Some(now);
            return true;
        }
        false
    }
}

pub struct Transport {
    prism: Arc<dyn PrismControl>,
    ytm: Arc<dyn YtmControl>,
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
}

impl Transport {
    pub fn new(player: PlayerHandle, ytm: Arc<dyn YtmControl>, clock: Arc<dyn Clock>) -> Self {
        Transport::with_control(Arc::new(player), ytm, clock)
    }

    pub fn with_control(
        prism: Arc<dyn PrismControl>,
        ytm: Arc<dyn YtmControl>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Transport {
            prism,
            ytm,
            clock,
            inner: Mutex::new(Inner {
                active: Active::Prism,
                prism: PlayerStatus::Idle,
                prism_paused: false,
                ytm_playing: false,
                ytm_paused_at: None,
                ytm_edge: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(inner) => inner,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn active(&self) -> Active {
        self.lock().active
    }

    /// Follows Prism's engine (see [`late_sink`]). Runs on the engine thread.
    pub fn on_player_event(&self, event: &PlayerEvent) {
        let PlayerEvent::State(state) = event else {
            return;
        };
        let pause_ytm = {
            let mut inner = self.lock();
            let was = std::mem::replace(&mut inner.prism, state.status);
            let started = matches!(was, PlayerStatus::Idle | PlayerStatus::Paused)
                && matches!(state.status, PlayerStatus::Loading | PlayerStatus::Playing);
            started && inner.prism_started(self.clock.now())
        };
        if pause_ytm {
            self.ytm.send(YtmAction::Pause);
        }
    }

    /// Follows YouTube Music's page, on every state the service derives from a report.
    pub fn on_ytm_state(&self, state: &YtmState) {
        let now = self.clock.now();
        let pause_prism = {
            let mut inner = self.lock();
            let playing = state.is_playing();
            let was = std::mem::replace(&mut inner.ytm_playing, playing);
            if !playing {
                inner.ytm_edge = None;
                return;
            }
            if !was {
                inner.ytm_edge = Some(now);
            }
            let Some(edge) = inner.ytm_edge else {
                return;
            };
            if inner.active == Active::Ytm {
                inner.ytm_edge = None;
                return;
            }
            let busy = inner.prism_busy();
            if busy {
                let expected = inner
                    .ytm_paused_at
                    .is_some_and(|at| now.saturating_duration_since(at) < EXPECTED_PAUSE);
                if expected || now.saturating_duration_since(edge) < YTM_DEBOUNCE {
                    return;
                }
            }
            inner.active = Active::Ytm;
            inner.ytm_edge = None;
            inner.ytm_paused_at = None;
            inner.prism_paused = busy;
            busy
        };
        if pause_prism {
            if let Err(err) = self.prism.pause() {
                eprintln!("pausing Prism for YouTube Music failed: {err}");
            }
        }
    }

    /// Prism is about to start from a user action that always means Prism (an idle click on
    /// the Flow ring): it becomes active now and the page pauses, before the engine reports.
    pub fn prism_starts(&self) {
        let pause_ytm = self.lock().prism_started(self.clock.now());
        if pause_ytm {
            self.ytm.send(YtmAction::Pause);
        }
    }

    fn route(
        &self,
        prism: impl FnOnce(&dyn PrismControl) -> Result<(), String>,
        ytm: YtmAction,
    ) -> Result<(), String> {
        match self.active() {
            Active::Prism => prism(self.prism.as_ref()),
            Active::Ytm => {
                self.ytm.send(ytm);
                Ok(())
            }
        }
    }

    pub fn toggle(&self) -> Result<(), String> {
        self.route(|prism| prism.toggle(), YtmAction::Toggle)
    }

    pub fn next(&self) -> Result<(), String> {
        self.route(|prism| prism.next(), YtmAction::Next)
    }

    pub fn prev(&self) -> Result<(), String> {
        self.route(|prism| prism.prev(), YtmAction::Prev)
    }

    pub fn seek(&self, ms: u64) -> Result<(), String> {
        self.route(|prism| prism.seek(ms), YtmAction::Seek(ms))
    }

    /// `volume` is clamped to 0..=1; anything that is not a number is refused.
    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("The volume must be a number between 0 and 1.".to_string());
        }
        self.route(
            |prism| prism.set_volume(volume),
            YtmAction::Volume(volume.clamp(0.0, 1.0)),
        )
    }
}

/// Feeds player events to the transport in `slot`. The player is spawned before the
/// transport (which needs its handle), so the slot is filled later; events before that are
/// dropped (the engine is idle then).
pub fn late_sink(slot: Arc<OnceLock<Arc<Transport>>>) -> EventSink {
    Arc::new(move |event| {
        if let Some(transport) = slot.get() {
            transport.on_player_event(&event);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{PlayerEvent, PlayerState, PlayerStatus};
    use crate::settings::RepeatMode;
    use crate::ytm::state::{YtmState, YtmStatus};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Toggle,
        Pause,
        Next,
        Prev,
        Seek(u64),
        Volume(f32),
    }

    #[derive(Default)]
    struct FakePrism {
        calls: Mutex<Vec<Call>>,
    }

    impl FakePrism {
        fn record(&self, call: Call) -> Result<(), String> {
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn pauses(&self) -> usize {
            self.calls().iter().filter(|c| **c == Call::Pause).count()
        }
    }

    impl PrismControl for FakePrism {
        fn toggle(&self) -> Result<(), String> {
            self.record(Call::Toggle)
        }
        fn pause(&self) -> Result<(), String> {
            self.record(Call::Pause)
        }
        fn next(&self) -> Result<(), String> {
            self.record(Call::Next)
        }
        fn prev(&self) -> Result<(), String> {
            self.record(Call::Prev)
        }
        fn seek(&self, ms: u64) -> Result<(), String> {
            self.record(Call::Seek(ms))
        }
        fn set_volume(&self, volume: f32) -> Result<(), String> {
            self.record(Call::Volume(volume))
        }
    }

    #[derive(Default)]
    struct FakeYtm {
        sent: Mutex<Vec<YtmAction>>,
    }

    impl FakeYtm {
        fn sent(&self) -> Vec<YtmAction> {
            self.sent.lock().unwrap().clone()
        }
        fn pauses(&self) -> usize {
            self.sent()
                .iter()
                .filter(|a| **a == YtmAction::Pause)
                .count()
        }
    }

    impl YtmControl for FakeYtm {
        fn send(&self, action: YtmAction) {
            self.sent.lock().unwrap().push(action);
        }
    }

    struct FakeClock {
        now: Mutex<Instant>,
    }

    impl FakeClock {
        fn advance(&self, ms: u64) {
            *self.now.lock().unwrap() += Duration::from_millis(ms);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }
    }

    struct Rig {
        transport: Arc<Transport>,
        prism: Arc<FakePrism>,
        ytm: Arc<FakeYtm>,
        clock: Arc<FakeClock>,
    }

    impl Rig {
        fn new() -> Rig {
            let prism = Arc::new(FakePrism::default());
            let ytm = Arc::new(FakeYtm::default());
            let clock = Arc::new(FakeClock {
                now: Mutex::new(Instant::now()),
            });
            let transport = Arc::new(Transport::with_control(
                Arc::clone(&prism) as Arc<dyn PrismControl>,
                Arc::clone(&ytm) as Arc<dyn YtmControl>,
                Arc::clone(&clock) as Arc<dyn Clock>,
            ));
            Rig {
                transport,
                prism,
                ytm,
                clock,
            }
        }

        fn prism(&self, status: PlayerStatus) {
            self.transport.on_player_event(&prism_state(status));
        }

        fn ytm(&self, status: YtmStatus) {
            self.transport.on_ytm_state(&ytm_state(status));
        }

        /// Prism starts a track and plays it.
        fn prism_plays(&self) {
            self.prism(PlayerStatus::Loading);
            self.prism(PlayerStatus::Playing);
        }

        /// YouTube Music starts while Prism is quiet: it takes over at once.
        fn ytm_plays(&self) {
            self.ytm(YtmStatus::Paused);
            self.ytm(YtmStatus::Playing);
            assert_eq!(self.transport.active(), Active::Ytm);
        }
    }

    fn prism_state(status: PlayerStatus) -> PlayerEvent {
        PlayerEvent::State(PlayerState {
            status,
            track: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 0.8,
            queue_index: 0,
            queue_length: 1,
            queue_version: 1,
            flow: None,
            repeat: RepeatMode::Off,
            shuffle: false,
        })
    }

    fn ytm_state(status: YtmStatus) -> YtmState {
        YtmState {
            status,
            title: "Fake Song".into(),
            artist: "Fake Artist".into(),
            ..YtmState::default()
        }
    }

    #[test]
    fn prism_is_active_at_first() {
        let rig = Rig::new();
        assert_eq!(rig.transport.active(), Active::Prism);
        rig.transport.toggle().unwrap();
        assert_eq!(rig.prism.calls(), vec![Call::Toggle]);
        assert!(rig.ytm.sent().is_empty());
    }

    #[test]
    fn ytm_start_pauses_prism() {
        let rig = Rig::new();
        rig.prism_plays();
        rig.ytm(YtmStatus::Paused);
        rig.ytm(YtmStatus::Playing);
        // A real edge, but Prism plays: the next report has to confirm it.
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.prism.pauses(), 0);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Ytm);
        assert_eq!(rig.prism.pauses(), 1);
        // The heartbeat goes on, Prism reports its pause: nothing more happens.
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Playing);
        rig.prism(PlayerStatus::Paused);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Ytm);
        assert_eq!(rig.prism.pauses(), 1);
        assert!(rig.ytm.sent().is_empty());
    }

    #[test]
    fn ytm_takes_over_at_once_while_prism_is_quiet() {
        let rig = Rig::new();
        rig.prism_plays();
        rig.prism(PlayerStatus::Paused);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Ytm);
        assert_eq!(rig.prism.pauses(), 0, "Prism was paused already");
    }

    #[test]
    fn a_blip_does_not_take_over() {
        let rig = Rig::new();
        rig.prism_plays();
        rig.ytm(YtmStatus::Playing);
        rig.clock.advance(100);
        rig.ytm(YtmStatus::Paused);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Paused);
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.prism.pauses(), 0);
    }

    #[test]
    fn prism_start_pauses_ytm() {
        let rig = Rig::new();
        rig.ytm_plays();
        rig.prism(PlayerStatus::Loading);
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.ytm.sent(), vec![YtmAction::Pause]);
        rig.prism(PlayerStatus::Playing);
        rig.prism(PlayerStatus::Playing);
        assert_eq!(rig.ytm.sent(), vec![YtmAction::Pause]);
        assert_eq!(rig.prism.pauses(), 0);
    }

    #[test]
    fn prism_resume_from_paused_pauses_ytm() {
        let rig = Rig::new();
        rig.prism_plays();
        rig.prism(PlayerStatus::Paused);
        rig.ytm_plays();
        // Play on a held station or a paused track: Paused -> Loading or Playing.
        rig.prism(PlayerStatus::Loading);
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.ytm.pauses(), 1);
    }

    #[test]
    fn own_pause_report_does_not_flip_active() {
        let rig = Rig::new();
        rig.ytm_plays();
        rig.prism_plays();
        assert_eq!(rig.ytm.pauses(), 1);
        // A report from before the pause arrived, then the pause, then the page flaps.
        rig.ytm(YtmStatus::Playing);
        rig.clock.advance(300);
        rig.ytm(YtmStatus::Paused);
        rig.clock.advance(300);
        rig.ytm(YtmStatus::Playing);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.prism.pauses(), 0);
        rig.ytm(YtmStatus::Paused);
        assert_eq!(rig.transport.active(), Active::Prism);
        // Long after the pause, a start in the page is the user's: it takes over.
        rig.clock.advance(2_000);
        rig.ytm(YtmStatus::Playing);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Ytm);
        assert_eq!(rig.prism.pauses(), 1);
        assert_eq!(rig.ytm.pauses(), 1);
    }

    #[test]
    fn a_start_inside_the_window_takes_over_once_it_has_passed() {
        let rig = Rig::new();
        rig.ytm_plays();
        rig.prism_plays();
        rig.ytm(YtmStatus::Paused);
        rig.clock.advance(500);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Prism);
        // Still playing after the window: a real start (the edge was real), so it wins.
        rig.clock.advance(1_600);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.transport.active(), Active::Ytm);
        assert_eq!(rig.prism.pauses(), 1);
    }

    #[test]
    fn simultaneous_starts_settle_on_the_last() {
        // YouTube Music's report first, then Prism's start: Prism wins.
        let rig = Rig::new();
        rig.ytm(YtmStatus::Paused);
        rig.ytm(YtmStatus::Playing);
        rig.prism(PlayerStatus::Loading);
        rig.ytm(YtmStatus::Playing);
        rig.prism(PlayerStatus::Playing);
        rig.ytm(YtmStatus::Paused);
        for _ in 0..5 {
            rig.clock.advance(1_000);
            rig.ytm(YtmStatus::Paused);
            rig.prism(PlayerStatus::Playing);
        }
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.ytm.pauses(), 1);
        assert_eq!(rig.prism.pauses(), 0);

        // Prism's start first, then YouTube Music's: YouTube Music wins.
        let rig = Rig::new();
        rig.ytm(YtmStatus::Paused);
        rig.prism(PlayerStatus::Loading);
        rig.ytm(YtmStatus::Playing);
        rig.prism(PlayerStatus::Playing);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Playing);
        rig.prism(PlayerStatus::Playing);
        rig.prism(PlayerStatus::Paused);
        for _ in 0..5 {
            rig.clock.advance(1_000);
            rig.ytm(YtmStatus::Playing);
            rig.prism(PlayerStatus::Paused);
        }
        assert_eq!(rig.transport.active(), Active::Ytm);
        assert_eq!(rig.prism.pauses(), 1);
        assert_eq!(rig.ytm.pauses(), 0);
    }

    #[test]
    fn commands_follow_the_active_player() {
        let rig = Rig::new();
        rig.transport.toggle().unwrap();
        rig.transport.next().unwrap();
        rig.transport.prev().unwrap();
        rig.transport.seek(5_000).unwrap();
        rig.transport.set_volume(0.5).unwrap();
        assert_eq!(
            rig.prism.calls(),
            vec![
                Call::Toggle,
                Call::Next,
                Call::Prev,
                Call::Seek(5_000),
                Call::Volume(0.5)
            ]
        );
        assert!(rig.ytm.sent().is_empty());

        rig.ytm_plays();
        rig.transport.toggle().unwrap();
        rig.transport.next().unwrap();
        rig.transport.prev().unwrap();
        rig.transport.seek(7_000).unwrap();
        rig.transport.set_volume(1.5).unwrap();
        assert!(rig.transport.set_volume(f32::NAN).is_err());
        assert_eq!(
            rig.ytm.sent(),
            vec![
                YtmAction::Toggle,
                YtmAction::Next,
                YtmAction::Prev,
                YtmAction::Seek(7_000),
                YtmAction::Volume(1.0)
            ]
        );
        assert_eq!(rig.prism.calls().len(), 5, "nothing more went to Prism");

        // Prism starts again: the commands follow it back.
        rig.prism(PlayerStatus::Loading);
        rig.transport.next().unwrap();
        assert_eq!(rig.prism.calls().last(), Some(&Call::Next));
    }

    #[test]
    fn flow_start_targets_prism() {
        let rig = Rig::new();
        rig.ytm_plays();
        // An idle click on the Flow ring: Prism, whatever was active.
        rig.transport.prism_starts();
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.ytm.sent(), vec![YtmAction::Pause]);
        // The engine's own start report pauses nothing a second time.
        rig.prism(PlayerStatus::Loading);
        rig.prism(PlayerStatus::Playing);
        assert_eq!(rig.ytm.sent(), vec![YtmAction::Pause]);
        rig.transport.toggle().unwrap();
        assert_eq!(rig.prism.calls(), vec![Call::Toggle]);
    }

    #[test]
    fn nothing_is_paused_twice() {
        let rig = Rig::new();
        rig.prism_plays();
        rig.ytm(YtmStatus::Playing);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.prism.pauses(), 1);
        // The user pauses and plays in the page before Prism's pause report arrives.
        rig.ytm(YtmStatus::Paused);
        rig.ytm(YtmStatus::Playing);
        rig.prism(PlayerStatus::Paused);
        rig.clock.advance(1_000);
        rig.ytm(YtmStatus::Paused);
        rig.ytm(YtmStatus::Playing);
        assert_eq!(rig.prism.pauses(), 1);

        // Prism takes over; the page never confirms the pause.
        rig.prism(PlayerStatus::Playing);
        assert_eq!(rig.ytm.pauses(), 1);
        rig.ytm(YtmStatus::Playing);
        // The user pauses and plays Prism again: still one pause for the page.
        rig.prism(PlayerStatus::Paused);
        rig.prism(PlayerStatus::Playing);
        rig.prism(PlayerStatus::Paused);
        rig.prism(PlayerStatus::Loading);
        rig.transport.prism_starts();
        assert_eq!(rig.ytm.pauses(), 1);
        assert_eq!(rig.transport.active(), Active::Prism);
    }

    #[test]
    fn late_sink_feeds_the_transport_once_set() {
        let rig = Rig::new();
        let slot: Arc<OnceLock<Arc<Transport>>> = Arc::default();
        let sink = late_sink(Arc::clone(&slot));
        // Before the transport exists, events are dropped (nothing panics).
        sink(prism_state(PlayerStatus::Loading));
        let _ = slot.set(Arc::clone(&rig.transport));
        rig.ytm_plays();
        sink(prism_state(PlayerStatus::Idle));
        sink(prism_state(PlayerStatus::Loading));
        assert_eq!(rig.transport.active(), Active::Prism);
        assert_eq!(rig.ytm.pauses(), 1);
    }
}
