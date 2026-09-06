use serde::{Deserialize, Serialize};

/// Remembered main-window geometry, in LOGICAL (DPI-independent)
/// units. Logical is the load-bearing choice: Wayland reports a
/// provisional scale factor while a window is being created, so a
/// size restored in physical pixels gets multiplied by the display
/// scale once the real factor arrives - the window literally grows
/// by e.g. 1.4x on every launch. Logical sizes round-trip exactly
/// no matter when the compositor settles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub width: f64,
    pub height: f64,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub maximized: bool,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        // Fails sane() on purpose: a row missing its size must fall
        // back to the conf.json default, not restore a 0x0 window.
        WindowGeometry {
            width: 0.0,
            height: 0.0,
            x: None,
            y: None,
            maximized: false,
        }
    }
}

/// One save cycle of the fixed-point size tracker.
///
/// The toolkit cannot be trusted to read back the same rectangle that
/// set_size sets: under GTK client-side decorations the reading
/// includes shadow and titlebar extents, so persisting raw readings
/// grows the window by the extents on every launch (measured +52 wide
/// +99 tall per cycle on KDE Wayland). Only the CHANGE relative to a
/// baseline reading is persisted: `baseline` is what the toolkit
/// reported for the size we last applied or saved, so
/// `current - baseline` is exactly the user's resizing since then and
/// any constant reporting offset cancels out.
///
/// Returns (new stored size, new baseline).
pub fn step(
    stored: Option<(f64, f64)>,
    baseline: Option<(f64, f64)>,
    current: (f64, f64),
) -> ((f64, f64), (f64, f64)) {
    match (stored, baseline) {
        (Some(t), Some(b)) => ((t.0 + (current.0 - b.0), t.1 + (current.1 - b.1)), current),
        // Stored size but no baseline captured this session (the
        // window was never focused unmaximized): nothing to measure a
        // delta against, keep what we have.
        (Some(t), None) => (t, current),
        // First save ever: record the raw reading. It includes the
        // extents once; every later cycle is delta-based and stable.
        (None, _) => (current, current),
    }
}

impl WindowGeometry {
    /// A corrupt or absurd row falls back to the conf.json default
    /// instead of restoring an unusable window.
    pub fn sane(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 400.0
            && self.height >= 300.0
            && self.width <= 20000.0
            && self.height <= 20000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(width: f64, height: f64) -> WindowGeometry {
        WindowGeometry {
            width,
            height,
            x: Some(10.0),
            y: Some(20.0),
            maximized: false,
        }
    }

    #[test]
    fn geometry_round_trips_through_json() {
        let g = geom(1640.0, 1020.0);
        let s = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<WindowGeometry>(&s).unwrap(), g);
    }

    #[test]
    fn missing_fields_deserialize_but_fail_the_sanity_check() {
        // Forward/backward compat: an old or hand-edited row parses,
        // and the zero-size default is rejected rather than applied.
        let g: WindowGeometry = serde_json::from_str("{}").unwrap();
        assert!(!g.sane());
        let g: WindowGeometry =
            serde_json::from_str(r#"{"width":1640.0,"height":1020.0}"#).unwrap();
        assert!(g.sane());
        assert_eq!(g.x, None);
    }

    #[test]
    fn garbage_is_rejected_not_applied() {
        assert!(serde_json::from_str::<WindowGeometry>("not json").is_err());
        assert!(!geom(f64::NAN, 1020.0).sane());
        assert!(!geom(f64::INFINITY, 1020.0).sane());
        assert!(!geom(-1640.0, 1020.0).sane());
        assert!(!geom(120.0, 80.0).sane());
        assert!(!geom(90000.0, 1020.0).sane());
    }

    #[test]
    fn constant_reporting_offset_never_grows_the_stored_size() {
        // The regression this module exists for: GTK CSD extents made
        // every launch cycle add +52x+99 before the delta tracking.
        let extents = (52.0, 99.0);
        let mut stored = (1640.0, 1020.0);
        for _ in 0..10 {
            // Launch: restore applies `stored`; the first focus-in
            // reads it back with the extents added - the baseline.
            let baseline = (stored.0 + extents.0, stored.1 + extents.1);
            // Focus-out with no user resize reads the same thing.
            let (next, _) = step(Some(stored), Some(baseline), baseline);
            stored = next;
        }
        assert_eq!(stored, (1640.0, 1020.0));
    }

    #[test]
    fn a_user_resize_moves_the_stored_size_by_exactly_the_resize() {
        let stored = (1640.0, 1020.0);
        let baseline = (1692.0, 1119.0);
        let dragged = (1892.0, 1219.0); // +200 x +100 from baseline
        let (next, next_baseline) = step(Some(stored), Some(baseline), dragged);
        assert_eq!(next, (1840.0, 1120.0));
        assert_eq!(next_baseline, dragged);
    }

    #[test]
    fn repeated_saves_in_one_session_do_not_double_count() {
        // Resize, alt-tab, alt-tab again: the second save measures
        // from the refreshed baseline and must not re-apply the delta.
        let (s1, b1) = step(
            Some((1640.0, 1020.0)),
            Some((1692.0, 1119.0)),
            (1792.0, 1219.0),
        );
        assert_eq!(s1, (1740.0, 1120.0));
        let (s2, _) = step(Some(s1), Some(b1), (1792.0, 1219.0));
        assert_eq!(s2, s1);
    }

    #[test]
    fn symmetric_platforms_track_the_reading_exactly() {
        // Zero extents (Windows/macOS): baseline equals stored, so
        // the stored size follows the user's window precisely.
        let (s, _) = step(
            Some((1000.0, 800.0)),
            Some((1000.0, 800.0)),
            (1234.0, 876.0),
        );
        assert_eq!(s, (1234.0, 876.0));
    }

    #[test]
    fn missing_pieces_degrade_to_something_harmless() {
        // No history at all: record the reading.
        assert_eq!(step(None, None, (1692.0, 1119.0)).0, (1692.0, 1119.0));
        // Stored but never focused this session: keep the stored size.
        assert_eq!(
            step(Some((1640.0, 1020.0)), None, (1892.0, 1219.0)).0,
            (1640.0, 1020.0)
        );
    }

    #[test]
    fn a_real_wayland_size_is_sane() {
        // The exact size the plugin bug produced (1640x1020 * 1.4)
        // is still a legal window; sanity bounds must not reject the
        // sizes users actually have.
        assert!(geom(2264.0, 1428.0).sane());
    }
}
