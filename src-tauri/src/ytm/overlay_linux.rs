//! Linux: the YouTube Music view floats over the main page.
//!
//! On Windows and macOS a child web view sits at its bounds over the window's main page. On
//! Linux tauri-runtime-wry packs every web view of a window into the window's vertical
//! `GtkBox`, one below the other, and wry applies bounds only inside a `GtkFixed`. The view
//! then took the lower part of the window and pushed the main page into a strip at the top. So
//! once the view exists, the main page moves into a `GtkOverlay` that takes its place in the
//! box, and the view becomes an overlay child: placed by its margins, sized by its size
//! request. Outside its allocation, input goes to the main page as before.

use gtk::prelude::*;

use super::view::Rect;

/// Floats `view` over the main page (moving it there the first time) and puts it at `rect`,
/// in logical pixels. The GTK work runs on the main thread (`with_webview`).
pub fn float(view: &tauri::Webview, rect: Rect) -> tauri::Result<()> {
    view.with_webview(move |platform| {
        let widget: gtk::Widget = platform.inner().upcast();
        match attach(&widget) {
            Ok(()) => place(&widget, rect),
            Err(err) => eprintln!("floating the YouTube Music view failed: {err}"),
        }
    })
}

/// Makes `widget` an overlay child over the main page, once.
fn attach(widget: &gtk::Widget) -> Result<(), &'static str> {
    let parent = widget.parent().ok_or("the view has no parent")?;
    if parent.is::<gtk::Overlay>() {
        return Ok(());
    }
    let vbox = parent
        .downcast::<gtk::Box>()
        .map_err(|_| "the view is not in the window's box")?;
    let children = vbox.children();
    // A view created again later finds the overlay from the first time.
    let overlay = match children
        .iter()
        .find_map(|child| child.clone().downcast::<gtk::Overlay>().ok())
    {
        Some(overlay) => overlay,
        None => {
            let main = children
                .iter()
                .find(|child| *child != widget && child.is::<webkit2gtk::WebView>())
                .ok_or("the main page is not in the window's box")?
                .clone();
            let overlay = gtk::Overlay::new();
            // `main` holds a reference while it is out of every container.
            vbox.remove(&main);
            overlay.add(&main);
            vbox.pack_start(&overlay, true, true, 0);
            overlay.show();
            main.show();
            overlay
        }
    };
    // Tauri decides when the view shows; moving it keeps that.
    let visible = widget.is_visible();
    vbox.remove(widget);
    overlay.add_overlay(widget);
    widget.set_halign(gtk::Align::Start);
    widget.set_valign(gtk::Align::Start);
    widget.set_visible(visible);
    Ok(())
}

/// Puts an overlay child at `rect`: the overlay honours the margins of a start-aligned child
/// and gives it its size request.
fn place(widget: &gtk::Widget, rect: Rect) {
    let (x, y, width, height) = pixels(rect);
    widget.set_margin_start(x);
    widget.set_margin_top(y);
    widget.set_size_request(width, height);
}

/// `rect` in whole pixels: never negative, at least 1 px wide and high (GTK reads a size
/// request of 0 as "no request").
fn pixels(rect: Rect) -> (i32, i32, i32, i32) {
    let px = |value: f64| {
        if value.is_finite() {
            value.round().clamp(0.0, f64::from(i32::MAX)) as i32
        } else {
            0
        }
    };
    (
        px(rect.x),
        px(rect.y),
        px(rect.width).max(1),
        px(rect.height).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_become_whole_pixels() {
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        assert_eq!(pixels(rect(292.6, 85.3, 986.7, 600.0)), (293, 85, 987, 600));
        assert_eq!(pixels(rect(-4.0, f64::NAN, 0.0, 0.2)), (0, 0, 1, 1));
    }
}
