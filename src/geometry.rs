use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub id: String,
    pub display: Display,
}

impl Display {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            [self.x, self.y, self.width, self.height, self.scale]
                .iter()
                .all(|v| v.is_finite()),
            "Invalid display geometry"
        );
        ensure!(
            (2.0..=32768.0).contains(&self.width) && (2.0..=32768.0).contains(&self.height),
            "Display dimensions outside supported range"
        );
        ensure!(
            self.x.abs() <= 32768.0
                && self.y.abs() <= 32768.0
                && (0.25..=8.0).contains(&self.scale),
            "Invalid display origin or scale"
        );
        Ok(self)
    }

    pub fn normalize(self, point: [f64; 2]) -> [f64; 2] {
        [
            (point[0] - self.x) / (self.width - 1.0),
            (point[1] - self.y) / (self.height - 1.0),
        ]
    }

    pub fn denormalize(self, point: [f64; 2]) -> [f64; 2] {
        [
            self.x + point[0] * (self.width - 1.0),
            self.y + point[1] * (self.height - 1.0),
        ]
    }

    pub fn on_edge(self, point: [f64; 2], side: Side) -> bool {
        let n = self.normalize(point);
        let (edge, along) = match side {
            Side::Left => (n[0], n[1]),
            Side::Right => (1. - n[0], n[1]),
            Side::Top => (n[1], n[0]),
            Side::Bottom => (1. - n[1], n[0]),
        };
        edge.abs() <= 1. / (self.width.min(self.height) - 1.) && (0. ..=1.).contains(&along)
    }
    pub fn is_outer_edge(self, bounds: Display, side: Side) -> bool {
        match side {
            Side::Left => self.x == bounds.x,
            Side::Right => self.x + self.width == bounds.x + bounds.width,
            Side::Top => self.y == bounds.y,
            Side::Bottom => self.y + self.height == bounds.y + bounds.height,
        }
    }
    pub fn contains(self, other: Display) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }
}

pub fn selected_monitor(monitors: &[Monitor], id: Option<&str>) -> Result<Display> {
    ensure!(!monitors.is_empty(), "No active monitors found");
    let monitor = id
        .map(|id| monitors.iter().find(|m| m.id == id))
        .unwrap_or_else(|| monitors.first())
        .ok_or_else(|| anyhow::anyhow!("Selected monitor is no longer active"))?;
    monitor.display.validate()
}

pub fn monitor_bounds(monitors: &[Monitor]) -> Result<Display> {
    ensure!(!monitors.is_empty(), "No active monitors found");
    let left = monitors
        .iter()
        .map(|m| m.display.x)
        .fold(f64::INFINITY, f64::min);
    let top = monitors
        .iter()
        .map(|m| m.display.y)
        .fold(f64::INFINITY, f64::min);
    let right = monitors
        .iter()
        .map(|m| m.display.x + m.display.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let bottom = monitors
        .iter()
        .map(|m| m.display.y + m.display.height)
        .fold(f64::NEG_INFINITY, f64::max);
    let bounds = Display {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
        scale: 1.,
    };
    ensure!(
        bounds.width.is_finite()
            && bounds.height.is_finite()
            && bounds.width > 1.
            && bounds.height > 1.,
        "Invalid monitor layout"
    );
    Ok(bounds)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
    pub fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub side: Side,
    /// Offset as a fraction of edge length; independent of DPI.
    pub alignment: f64,
}

impl Layout {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            self.alignment.is_finite() && self.alignment.abs() <= 0.9,
            "Alignment must be between -90% and 90%"
        );
        Ok(self)
    }
    pub fn reciprocal(self) -> Self {
        Self {
            side: self.side.opposite(),
            alignment: -self.alignment,
        }
    }
    pub fn enter(self, point: [f64; 2]) -> Option<[f64; 2]> {
        let along = point[usize::from(self.side.horizontal())] - self.alignment;
        if !(0.0..=1.0).contains(&along) {
            return None;
        }
        let inset = 0.002;
        Some(match self.side {
            Side::Left => [1.0 - inset, along],
            Side::Right => [inset, along],
            Side::Top => [along, 1.0 - inset],
            Side::Bottom => [along, inset],
        })
    }
    pub fn returns(self, point: [f64; 2]) -> bool {
        match self.side {
            Side::Left => point[0] >= 1.0,
            Side::Right => point[0] <= 0.0,
            Side::Top => point[1] >= 1.0,
            Side::Bottom => point[1] <= 0.0,
        }
    }
    pub fn return_point(self, point: [f64; 2]) -> Option<[f64; 2]> {
        self.reciprocal().enter(point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_handoff_monitor_uses_its_own_coordinates() {
        let primary = Monitor {
            id: "primary".into(),
            display: Display {
                x: 0.,
                y: 0.,
                width: 1000.,
                height: 800.,
                scale: 1.,
            },
        };
        let right = Monitor {
            id: "right".into(),
            display: Display {
                x: 1000.,
                y: -100.,
                width: 1200.,
                height: 900.,
                scale: 1.,
            },
        };
        let monitors = [primary, right];
        let selected = selected_monitor(&monitors, Some("right")).unwrap();
        assert_eq!(selected.normalize([2199., 349.5]), [1., 0.5]);
        assert!(selected.on_edge([2199., 349.5], Side::Right));
        assert!(!selected.on_edge([999., 350.], Side::Right));
        assert!(
            !monitors[0]
                .display
                .is_outer_edge(monitor_bounds(&monitors).unwrap(), Side::Right)
        );
        assert!(selected.is_outer_edge(monitor_bounds(&monitors).unwrap(), Side::Right));
        assert_eq!(selected.denormalize([0., 0.5]), [1000., 349.5]);
        let desktop = monitor_bounds(&monitors).unwrap();
        let mapped = desktop.denormalize(desktop.normalize(selected.denormalize([0., 0.5])));
        assert!((mapped[0] - 1000.).abs() < 1e-8 && (mapped[1] - 349.5).abs() < 1e-8);
        assert_eq!(
            monitor_bounds(&monitors).unwrap(),
            Display {
                x: 0.,
                y: -100.,
                width: 2200.,
                height: 900.,
                scale: 1.
            }
        );
        assert!(selected_monitor(&monitors, Some("removed")).is_err());
    }
    #[test]
    fn negative_origin_and_scaling_use_logical_coordinates() {
        let d = Display {
            x: -1920.,
            y: -200.,
            width: 1920.,
            height: 1080.,
            scale: 1.5,
        }
        .validate()
        .unwrap();
        let p = [-1200., 100.];
        let actual = d.denormalize(d.normalize(p));
        assert!((p[0] - actual[0]).abs() < 1e-8 && (p[1] - actual[1]).abs() < 1e-8);
        assert!(
            Display {
                width: f64::NAN,
                ..d
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn all_edges_roundtrip_with_alignment_and_unequal_dimensions() {
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            let l = Layout {
                side,
                alignment: 0.2,
            };
            let p = [0.6, 0.7];
            let remote = l.enter(p).unwrap();
            let back = l.return_point(remote).unwrap();
            let axis = usize::from(side.horizontal());
            assert!((back[axis] - p[axis]).abs() < 1e-8);
            assert_eq!(l.reciprocal().reciprocal(), l);
        }
        assert!(
            Layout {
                side: Side::Right,
                alignment: 0.5
            }
            .enter([1., 0.2])
            .is_none()
        );
    }
}

#[cfg(test)]
mod overlap_tests {
    use super::*;
    #[test]
    fn nonoverlapping_return_edge_does_not_teleport_to_center() {
        let layout = Layout {
            side: Side::Right,
            alignment: 0.5,
        };
        assert!(layout.return_point([0.0, 0.8]).is_none());
        assert!(layout.return_point([0.0, 0.2]).is_some());
    }
}
