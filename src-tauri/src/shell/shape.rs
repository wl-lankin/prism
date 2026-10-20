//! The clickable shape of a transparent window.
//!
//! The main window is a card with a transparent strip below it, and the Flow ring hangs half
//! out of the card into that strip. Clicks on the strip beside the ring must reach whatever
//! is behind the window, so the window's shape is the card plus the ring (with its bezel and
//! glow). On Windows that is a window region (`SetWindowRgn`); on macOS a tracker thread
//! toggles `set_ignore_cursor_events` as the cursor enters and leaves the shape.
//!
//! The numbers come from `src/lib/window-shape.json`, which the page reads too.

use std::sync::LazyLock;

use serde::Deserialize;
use tauri::WebviewWindow;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MainShape {
    pub strip: f64,
    pub ring: f64,
    pub ring_room: f64,
    pub corner: f64,
    pub glow: f64,
    pub ring_maximized: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MiniShape {
    pub pad_left: f64,
    pub pad_y: f64,
    pub width: f64,
    pub height: f64,
    pub ring: f64,
    pub corner: f64,
    pub glow: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shapes {
    pub main: MainShape,
    pub mini: MiniShape,
}

static SHAPES: LazyLock<Shapes> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/lib/window-shape.json"))
        .expect("src/lib/window-shape.json is valid")
});

/// The window shapes shared with the page (`src/lib/window-shape.json`), in logical px.
pub fn shapes() -> &'static Shapes {
    &SHAPES
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub card: RectF,
    pub corner: f64,
    pub ring: Circle,
}

/// The bezel the page draws around the ring (`box-shadow: 0 0 0 6px`).
const BEZEL: f64 = 6.0;

/// The main window of `width` by `height` logical px: the card fills all but the strip at
/// the bottom, and the ring with its bezel and glow is centred on the card's bottom edge.
pub fn main_region(width: f64, height: f64) -> Region {
    let main = shapes().main;
    let bottom = (height - main.strip).max(0.0);
    Region {
        card: RectF {
            x: 0.0,
            y: 0.0,
            w: width,
            h: bottom,
        },
        corner: main.corner,
        ring: Circle {
            cx: width / 2.0,
            cy: bottom,
            r: main.ring / 2.0 + BEZEL + main.glow,
        },
    }
}

/// The mini player: the card sits `padLeft` in and `padY` down, and the ring is centred on
/// its left edge.
pub fn mini_region() -> Region {
    let mini = shapes().mini;
    Region {
        card: RectF {
            x: mini.pad_left,
            y: mini.pad_y,
            w: mini.width,
            h: mini.height,
        },
        corner: mini.corner,
        ring: Circle {
            cx: mini.pad_left,
            cy: mini.pad_y + mini.height / 2.0,
            r: mini.ring / 2.0 + BEZEL + mini.glow,
        },
    }
}

impl Region {
    /// True when the point (window coordinates, same unit as the region) is in the card or
    /// in the ring's circle.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.in_card(x, y) || self.in_ring(x, y)
    }

    fn in_card(&self, x: f64, y: f64) -> bool {
        let RectF {
            x: left,
            y: top,
            w,
            h,
        } = self.card;
        let (right, bottom) = (left + w, top + h);
        if x < left || y < top || x > right || y > bottom {
            return false;
        }
        let c = self.corner.min(w / 2.0).min(h / 2.0).max(0.0);
        let nx = x.clamp(left + c, right - c);
        let ny = y.clamp(top + c, bottom - c);
        (x - nx).powi(2) + (y - ny).powi(2) <= c * c
    }

    fn in_ring(&self, x: f64, y: f64) -> bool {
        let Circle { cx, cy, r } = self.ring;
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    }

    /// The same region in physical px.
    pub fn to_physical(&self, scale: f64) -> Region {
        Region {
            card: RectF {
                x: self.card.x * scale,
                y: self.card.y * scale,
                w: self.card.w * scale,
                h: self.card.h * scale,
            },
            corner: self.corner * scale,
            ring: Circle {
                cx: self.ring.cx * scale,
                cy: self.ring.cy * scale,
                r: self.ring.r * scale,
            },
        }
    }
}

/// Gives `window` the shape `region` (logical px, window coordinates); `None` makes the whole
/// window clickable again.
///
/// Windows: a window region, the card's rounded rectangle OR the ring's circle, in physical
/// px. Outside it the window neither paints nor takes clicks.
#[cfg(target_os = "windows")]
pub fn apply(window: &WebviewWindow, region: Option<&Region>) {
    use windows_sys::Win32::Graphics::Gdi::{
        CombineRgn, CreateEllipticRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, HRGN,
        RGN_ERROR, RGN_OR,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = hwnd.0 as _;
    let rgn: HRGN = match region {
        None => std::ptr::null_mut(),
        Some(region) => {
            let scale = window.scale_factor().unwrap_or(1.0);
            let Region { card, corner, ring } = region.to_physical(scale);
            let px = |v: f64| v.round() as i32;
            let ellipse = px(corner * 2.0);
            // SAFETY: plain GDI region calls; every handle created here is either handed to
            // the window (which then owns it) or deleted below.
            unsafe {
                // GDI leaves out the right column and bottom row of these shapes, so the
                // far edges get one more pixel; otherwise the card's 1 px edge is cut off.
                // At fractional scales WebView2 lays the page out a pixel or two taller
                // than the client area (175 %: the card ends 2 px below its computed
                // edge), so the card's bottom also gets `ceil(scale)` px of slack.
                let slack = scale.ceil() as i32;
                let card_rgn = CreateRoundRectRgn(
                    px(card.x),
                    px(card.y),
                    px(card.x + card.w) + 1,
                    px(card.y + card.h) + 1 + slack,
                    ellipse,
                    ellipse,
                );
                let ring_rgn = CreateEllipticRgn(
                    px(ring.cx - ring.r),
                    px(ring.cy - ring.r),
                    px(ring.cx + ring.r) + 1,
                    px(ring.cy + ring.r) + 1,
                );
                if card_rgn.is_null() || ring_rgn.is_null() {
                    eprintln!("creating the window region failed");
                    for rgn in [card_rgn, ring_rgn] {
                        if !rgn.is_null() {
                            DeleteObject(rgn);
                        }
                    }
                    return;
                }
                let combined = CombineRgn(card_rgn, card_rgn, ring_rgn, RGN_OR);
                DeleteObject(ring_rgn);
                if combined == RGN_ERROR {
                    eprintln!("combining the window region failed");
                    DeleteObject(card_rgn);
                    return;
                }
                card_rgn
            }
        }
    };
    // SAFETY: `hwnd` is the live window. On success the system owns `rgn`; on failure it is
    // still ours to delete.
    unsafe {
        if SetWindowRgn(hwnd, rgn, 1) == 0 {
            eprintln!("SetWindowRgn failed");
            if !rgn.is_null() {
                DeleteObject(rgn);
            }
        }
    }
}

/// macOS has no window regions: a tracker thread per window polls the cursor every 30 ms
/// while the window is visible and lets clicks through (`set_ignore_cursor_events`) whenever
/// the cursor is outside the shape. It stops when the window hides or the shape is removed;
/// the next `apply` starts it again.
#[cfg(target_os = "macos")]
pub fn apply(window: &WebviewWindow, region: Option<&Region>) {
    tracker::apply(window, region.copied());
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn apply(_window: &WebviewWindow, _region: Option<&Region>) {}

#[cfg(target_os = "macos")]
mod tracker {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::Duration;

    use tauri::{Manager, WebviewWindow};

    use super::Region;

    const POLL: Duration = Duration::from_millis(30);

    #[derive(Default)]
    struct Tracked {
        region: Mutex<Option<Region>>,
        running: AtomicBool,
    }

    static TRACKED: LazyLock<Mutex<HashMap<String, Arc<Tracked>>>> =
        LazyLock::new(Default::default);

    pub fn apply(window: &WebviewWindow, region: Option<Region>) {
        let tracked = {
            let Ok(mut all) = TRACKED.lock() else {
                return;
            };
            Arc::clone(all.entry(window.label().to_string()).or_default())
        };
        if let Ok(mut current) = tracked.region.lock() {
            *current = region;
        }
        if region.is_none() {
            let _ = window.set_ignore_cursor_events(false);
            return;
        }
        if tracked.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let window = window.clone();
        std::thread::spawn(move || {
            track(&window, &tracked);
            let _ = window.set_ignore_cursor_events(false);
            tracked.running.store(false, Ordering::Release);
        });
    }

    fn track(window: &WebviewWindow, tracked: &Tracked) {
        let mut ignoring = false;
        loop {
            std::thread::sleep(POLL);
            let Some(region) = tracked.region.lock().ok().and_then(|region| *region) else {
                return;
            };
            if !window.is_visible().unwrap_or(false) {
                return;
            }
            let (Ok(cursor), Ok(origin), Ok(scale)) = (
                window.app_handle().cursor_position(),
                window.inner_position(),
                window.scale_factor(),
            ) else {
                continue;
            };
            let x = (cursor.x - f64::from(origin.x)) / scale;
            let y = (cursor.y - f64::from(origin.y)) / scale;
            let ignore = !region.contains(x, y);
            if ignore != ignoring && window.set_ignore_cursor_events(ignore).is_ok() {
                ignoring = ignore;
            }
        }
    }
}

/// Re-applies the main window's shape for its current size: the card plus the ring, or no
/// shape at all while it is maximized or fullscreen (there is no strip then). A minimized
/// window keeps its shape.
pub fn reshape_main(window: &WebviewWindow) {
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let full = window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false);
    if full {
        apply(window, None);
        return;
    }
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    apply(window, Some(&main_region(size.width, size.height)));
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f64 = 1280.0;
    const H: f64 = 820.0;

    #[test]
    fn card_and_ring_are_inside() {
        let region = main_region(W, H);
        let strip = shapes().main.strip;
        // The middle of the card and a point just above its bottom edge, far from the ring.
        assert!(region.contains(W / 2.0, H / 2.0));
        assert!(region.contains(40.0, H - strip - 2.0));
        // The ring's lower half hangs below the card, over the strip.
        assert!(region.contains(W / 2.0, H - strip + 30.0));
        assert!(region.contains(W / 2.0 + 30.0, H - strip + 20.0));
        // The glow below the ring is part of the shape too, out to where it fades (88 px).
        assert!(region.contains(W / 2.0, H - strip + 88.0));
        assert!(region.contains(W / 2.0 + 62.0, H - strip + 62.0));
    }

    #[test]
    fn strip_corners_are_outside() {
        let region = main_region(W, H);
        let strip = shapes().main.strip;
        assert!(!region.contains(5.0, H - 5.0));
        assert!(!region.contains(W - 5.0, H - 5.0));
        // Beside the ring in the strip.
        assert!(!region.contains(W / 2.0 - 200.0, H - strip + 20.0));
        assert!(!region.contains(W / 2.0 + 200.0, H - 10.0));
        // The card's own rounded corners are cut off.
        assert!(!region.contains(0.5, 0.5));
        assert!(!region.contains(W - 0.5, H - strip - 0.5));
        // Outside the window.
        assert!(!region.contains(-1.0, H / 2.0));
        assert!(!region.contains(W + 1.0, H / 2.0));
    }

    #[test]
    fn main_region_follows_the_shared_numbers() {
        let main = shapes().main;
        let region = main_region(W, H);
        assert_eq!(
            region.card,
            RectF {
                x: 0.0,
                y: 0.0,
                w: W,
                h: H - main.strip
            }
        );
        assert_eq!(region.corner, main.corner);
        assert_eq!(
            region.ring,
            Circle {
                cx: W / 2.0,
                cy: H - main.strip,
                r: main.ring / 2.0 + 6.0 + main.glow
            }
        );
        // The ring and its glow fit in the strip, so the window edge does not cut them.
        assert!(region.ring.r <= main.strip);
    }

    #[test]
    fn mini_region_puts_the_ring_on_the_left_edge() {
        let mini = shapes().mini;
        let region = mini_region();
        assert_eq!(
            region.card,
            RectF {
                x: mini.pad_left,
                y: mini.pad_y,
                w: mini.width,
                h: mini.height
            }
        );
        assert_eq!(region.ring.cx, mini.pad_left);
        assert_eq!(region.ring.cy, mini.pad_y + mini.height / 2.0);
        assert_eq!(region.ring.r, mini.ring / 2.0 + 6.0 + mini.glow);
        assert!(region.contains(mini.pad_left - 30.0, mini.pad_y + mini.height / 2.0));
        assert!(!region.contains(1.0, 1.0));
    }

    #[test]
    fn region_scales_with_dpi() {
        let logical = main_region(W, H);
        let physical = logical.to_physical(1.5);
        assert_eq!(
            physical.card,
            RectF {
                x: 0.0,
                y: 0.0,
                w: W * 1.5,
                h: logical.card.h * 1.5
            }
        );
        assert_eq!(physical.corner, logical.corner * 1.5);
        assert_eq!(
            physical.ring,
            Circle {
                cx: logical.ring.cx * 1.5,
                cy: logical.ring.cy * 1.5,
                r: logical.ring.r * 1.5
            }
        );
        let points = [
            (W / 2.0, H / 2.0),
            (5.0, H - 5.0),
            (W / 2.0, H - 30.0),
            (W / 2.0 - 200.0, H - 20.0),
            (0.5, 0.5),
            (W / 2.0 + 70.0, H - 60.0),
        ];
        for (x, y) in points {
            assert_eq!(
                physical.contains(x * 1.5, y * 1.5),
                logical.contains(x, y),
                "({x}, {y})"
            );
        }
    }

    #[test]
    fn shared_json_matches_constants() {
        let parsed: Shapes =
            serde_json::from_str(include_str!("../../../src/lib/window-shape.json")).unwrap();
        assert_eq!(&parsed, shapes());
        assert_eq!(
            parsed.main,
            MainShape {
                strip: 96.0,
                ring: 96.0,
                ring_room: 48.0,
                corner: 16.0,
                // Measured on screen: the 96 px ring's blurred glow fades out about 88 px
                // from its centre, so 48 + 6 + 36 = 90 keeps all of it.
                glow: 36.0,
                ring_maximized: 72.0
            }
        );
        // The strip is half a ring plus the room below it.
        assert_eq!(
            parsed.main.strip,
            parsed.main.ring / 2.0 + parsed.main.ring_room
        );
        assert_eq!(
            parsed.mini,
            MiniShape {
                pad_left: 66.0,
                pad_y: 22.0,
                width: 310.0,
                height: 92.0,
                ring: 76.0,
                corner: 18.0,
                glow: 22.0
            }
        );
    }
}
