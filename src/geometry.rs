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
    pub fn return_point(self, point: [f64; 2]) -> [f64; 2] {
        self.reciprocal().enter(point).unwrap_or([0.5, 0.5])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let back = l.return_point(remote);
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
