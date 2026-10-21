//! Counting clicks: a press that follows another closely enough, in time and place, is a double
//! click, and the next a triple. Used by the document's selection and by the find bar's field,
//! each with a count of its own.

use std::time::{Duration, Instant};

/// Presses within this time, and the drag threshold of one another, are a double or triple
/// click. winit gives no click count, and neither X11 nor Wayland tells a client the desktop's
/// interval; this is the Windows default, at or above the GTK and KDE ones.
pub(super) const MULTI_CLICK: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug)]
struct Click {
    at: (f64, f64),
    when: Instant,
    count: u8,
}

/// The press that started the last gesture, for counting clicks.
#[derive(Debug, Default)]
pub(super) struct Clicks {
    last: Option<Click>,
}

impl Clicks {
    /// A press that starts nothing: it isn't part of a double click.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// The count of this press: 1, 2 or 3 for a click, double click or triple click; a fourth
    /// begins again.
    pub fn count(&mut self, now: Instant, at: (f64, f64), slop: f64) -> u8 {
        let n = match self.last {
            Some(c)
                if now.saturating_duration_since(c.when) <= MULTI_CLICK
                    && (at.0 - c.at.0).hypot(at.1 - c.at.1) <= slop
                    && c.count < 3 =>
            {
                c.count + 1
            }
            _ => 1,
        };
        self.last = Some(Click {
            at,
            when: now,
            count: n,
        });
        n
    }
}
