//! The screen arithmetic every platform's capture shares: a device's
//! displays as zones in one layout space, where along an edge of the whole
//! layout a pointer is, and where to put it back when a drive returns.

use lp_core::input::Edge;

/// One display: its rectangle in the layout's space (logical pixels on
/// Linux, points on macOS), the origin at the top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    /// Left, in layout pixels.
    pub x: i32,
    /// Top.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// The layout's bounding box: left, top, right and bottom, the right and
/// bottom exclusive. An empty layout is the unit square.
#[must_use]
pub fn bounds(zones: &[Zone]) -> (i32, i32, i32, i32) {
    let left = zones.iter().map(|z| z.x).min().unwrap_or(0);
    let top = zones.iter().map(|z| z.y).min().unwrap_or(0);
    let right = zones
        .iter()
        .map(|z| z.x + z.width as i32)
        .max()
        .unwrap_or(1);
    let bottom = zones
        .iter()
        .map(|z| z.y + z.height as i32)
        .max()
        .unwrap_or(1);
    (left, top, right, bottom)
}

/// The layout's size, for `ready` and for scaling.
#[must_use]
pub fn size(zones: &[Zone]) -> (u32, u32) {
    let (left, top, right, bottom) = bounds(zones);
    ((right - left).max(0) as u32, (bottom - top).max(0) as u32)
}

/// Where along `edge` of the whole layout `cursor` is, as a fraction from
/// its top or left end.
#[must_use]
pub fn edge_fraction(edge: Edge, zones: &[Zone], cursor: (f64, f64)) -> f64 {
    let (left, top, right, bottom) = bounds(zones);
    match edge {
        Edge::Left | Edge::Right => {
            ((cursor.1 - f64::from(top)) / f64::from(bottom - top).max(1.0)).clamp(0.0, 1.0)
        }
        Edge::Top | Edge::Bottom => {
            ((cursor.0 - f64::from(left)) / f64::from(right - left).max(1.0)).clamp(0.0, 1.0)
        }
    }
}

/// Where to put the cursor back when the pointer returns by `edge` at `at`:
/// just inside the layout's edge.
#[must_use]
pub fn back_position(edge: Edge, at: f64, zones: &[Zone]) -> (f64, f64) {
    let (left, top, right, bottom) = bounds(zones);
    let at = at.clamp(0.0, 1.0);
    let x_along = f64::from(left) + at * f64::from(right - left).max(1.0);
    let y_along = f64::from(top) + at * f64::from(bottom - top).max(1.0);
    match edge {
        Edge::Left => (f64::from(left) + 2.0, y_along),
        Edge::Right => (f64::from(right) - 3.0, y_along),
        Edge::Top => (x_along, f64::from(top) + 2.0),
        Edge::Bottom => (x_along, f64::from(bottom) - 3.0),
    }
}

/// A point of the layout as fractions of its bounding box.
#[must_use]
pub fn point_at(zones: &[Zone], fraction: (f64, f64)) -> (f64, f64) {
    let (left, top, right, bottom) = bounds(zones);
    let x = f64::from(left) + fraction.0.clamp(0.0, 1.0) * f64::from(right - left - 1).max(0.0);
    let y = f64::from(top) + fraction.1.clamp(0.0, 1.0) * f64::from(bottom - top - 1).max(0.0);
    (x, y)
}

/// `point` moved onto the nearest display: unchanged when a display holds
/// it, else clamped into the display it is closest to, so a pointer pushed
/// into a gap of an irregular layout stays on a screen.
#[must_use]
pub fn clamp_to_zones(zones: &[Zone], point: (f64, f64)) -> (f64, f64) {
    let inside = |z: &Zone| -> (f64, f64) {
        let x2 = f64::from(z.x) + f64::from(z.width) - 1.0;
        let y2 = f64::from(z.y) + f64::from(z.height) - 1.0;
        (
            point.0.clamp(f64::from(z.x), x2.max(f64::from(z.x))),
            point.1.clamp(f64::from(z.y), y2.max(f64::from(z.y))),
        )
    };
    zones
        .iter()
        .map(inside)
        .min_by(|a, b| {
            let da = (a.0 - point.0).powi(2) + (a.1 - point.1).powi(2);
            let db = (b.0 - point.0).powi(2) + (b.1 - point.1).powi(2);
            da.total_cmp(&db)
        })
        .unwrap_or(point)
}

/// Which armed edge of the layout `cursor` lies on, if any: the cursor sits
/// on the layout's outermost pixel row or column. A corner counts for the
/// first armed edge it touches.
#[must_use]
pub fn edge_under(zones: &[Zone], armed: &[Edge], cursor: (f64, f64)) -> Option<Edge> {
    if zones.is_empty() {
        return None;
    }
    let (left, top, right, bottom) = bounds(zones);
    armed.iter().copied().find(|edge| match edge {
        Edge::Left => cursor.0 < f64::from(left) + 1.0,
        Edge::Right => cursor.0 >= f64::from(right) - 1.0,
        Edge::Top => cursor.1 < f64::from(top) + 1.0,
        Edge::Bottom => cursor.1 >= f64::from(bottom) - 1.0,
    })
}

/// Turns cursor positions into edge hits for a capture that watches the
/// cursor rather than being told by the compositor (macOS): a hit is
/// reported once when the cursor arrives on an armed edge, and again only
/// after it has left that edge, so a pointer resting there while no drive
/// can start does not repeat.
#[derive(Debug, Default)]
pub struct EdgeWatch {
    on: Option<Edge>,
}

impl EdgeWatch {
    /// The cursor is at `cursor`: the edge it just arrived on, with where
    /// along it, or `None`.
    pub fn update(
        &mut self,
        zones: &[Zone],
        armed: &[Edge],
        cursor: (f64, f64),
    ) -> Option<(Edge, f64)> {
        let under = edge_under(zones, armed, cursor);
        let arrived = under.filter(|e| self.on != Some(*e));
        self.on = under;
        arrived.map(|edge| (edge, edge_fraction(edge, zones, cursor)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(x: i32, y: i32, w: u32, h: u32) -> Zone {
        Zone {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn edges_fractions_and_the_way_back_are_of_the_whole_layout() {
        let zones = [zone(0, 0, 1920, 1080), zone(1920, 0, 2560, 1440)];
        assert_eq!(size(&zones), (4480, 1440));
        assert!((edge_fraction(Edge::Right, &zones, (4479.0, 720.0)) - 0.5).abs() < 1e-9);
        assert!((edge_fraction(Edge::Top, &zones, (2240.0, 0.0)) - 0.5).abs() < 1e-9);
        assert_eq!(back_position(Edge::Right, 0.5, &zones), (4477.0, 720.0));
        assert_eq!(back_position(Edge::Left, 0.0, &zones), (2.0, 0.0));
        assert_eq!(back_position(Edge::Bottom, 1.0, &zones), (4480.0, 1437.0));
        assert_eq!(point_at(&zones, (0.0, 0.0)), (0.0, 0.0));
        assert_eq!(point_at(&zones, (1.0, 1.0)), (4479.0, 1439.0));
    }

    #[test]
    fn a_point_in_a_gap_of_the_layout_lands_on_the_nearest_display() {
        // A laptop below the left half of a wider monitor.
        let zones = [zone(0, 0, 2560, 1440), zone(0, 1440, 1440, 900)];
        assert_eq!(clamp_to_zones(&zones, (100.0, 100.0)), (100.0, 100.0));
        assert_eq!(clamp_to_zones(&zones, (2000.0, 1800.0)), (2000.0, 1439.0));
        assert_eq!(clamp_to_zones(&zones, (-50.0, 2000.0)), (0.0, 2000.0));
        assert_eq!(clamp_to_zones(&[], (5.0, 5.0)), (5.0, 5.0));
    }

    #[test]
    fn the_watch_reports_an_arrival_once_and_again_after_leaving() {
        let zones = [zone(0, 0, 1000, 500)];
        let armed = [Edge::Right, Edge::Top];
        let mut watch = EdgeWatch::default();
        assert_eq!(watch.update(&zones, &armed, (500.0, 250.0)), None);
        let hit = watch.update(&zones, &armed, (999.0, 250.0)).unwrap();
        assert_eq!(hit.0, Edge::Right);
        assert!((hit.1 - 0.5).abs() < 1e-9);
        assert_eq!(
            watch.update(&zones, &armed, (999.0, 300.0)),
            None,
            "resting"
        );
        assert_eq!(watch.update(&zones, &armed, (990.0, 300.0)), None);
        assert!(
            watch.update(&zones, &armed, (999.5, 100.0)).is_some(),
            "back again"
        );
        // The left and bottom edges are not armed.
        assert_eq!(watch.update(&zones, &armed, (0.0, 250.0)), None);
        assert_eq!(watch.update(&zones, &armed, (500.0, 499.0)), None);
        assert_eq!(
            watch.update(&zones, &armed, (500.0, 0.0)).map(|h| h.0),
            Some(Edge::Top)
        );
        assert_eq!(watch.update(&[], &armed, (0.0, 0.0)), None);
    }
}
