//! Cosmetic "glass break" effect played when the cursor crosses to or from another device.
//!
//! Designed to look crisp and lively while costing nothing at idle:
//! - Fan-shaped inward hemisphere so 100% of fractures and shards stay on-screen.
//! - Multi-segment jagged spiderweb cracks with a cyan glow under-stroke and white core.
//! - Strictly triangular crystalline shards (guaranteed convex) with 2D spin, 3D faux-tumble,
//!   gravity, prismatic ice tints, and specular light glints.
//! - Twinkling 4-point star sparkles (`✦`) for a cute, polished finish.

use crate::geometry::Side;
use eframe::egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use std::f32::consts::{FRAC_PI_2, PI};
use std::time::Duration;

pub const DURATION: Duration = Duration::from_millis(780);
pub const OVERLAY_SIZE: f32 = 460.0;

const SECTORS: usize = 10;
const RINGS: usize = 3;
const SPARKLES: usize = 14;

/// Deterministic pseudo-random in `0.0..=1.0`.
#[inline]
fn hash(i: usize, salt: u32) -> f32 {
    let mut x =
        (i as u32).wrapping_add(1).wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Signed pseudo-random in `-1.0..=1.0`.
#[inline]
fn shash(i: usize, salt: u32) -> f32 {
    hash(i, salt) * 2.0 - 1.0
}

#[inline]
fn u8_alpha(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[inline]
fn ease_out_cubic(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

/// Inward screen unit normal and tangent for the given edge `side`.
pub fn basis(side: Side) -> (Vec2, Vec2) {
    match side {
        Side::Left => (vec2(1.0, 0.0), vec2(0.0, 1.0)),
        Side::Right => (vec2(-1.0, 0.0), vec2(0.0, 1.0)),
        Side::Top => (vec2(0.0, 1.0), vec2(1.0, 0.0)),
        Side::Bottom => (vec2(0.0, -1.0), vec2(1.0, 0.0)),
    }
}

/// Draw one frame of the glass-break animation.
///
/// `origin` is the impact point on `side` of `screen`, and `t` is normalized time in `0.0..=1.0`.
pub fn paint(shapes: &mut Vec<Shape>, screen: Rect, origin: Pos2, side: Side, t: f32) {
    let t = t.clamp(0.0, 1.0);
    let min_dim = screen.width().min(screen.height()).max(64.0);
    // Scale smoothly from tiny preview canvases (~200px) up to full overlay (~460px+).
    let reach = (min_dim * 0.44).clamp(52.0, 210.0);
    let scale = (reach / 180.0).clamp(0.45, 1.25);
    let (normal, tangent) = basis(side);

    // Build a deterministic radial lattice of nodes in the inward hemisphere.
    // Ring 0 is `origin`; rings 1..=RINGS fan across -78°..+78° around `normal`.
    let ring_fracs: [f32; RINGS + 1] = [0.0, 0.24, 0.56, 1.0];
    let node = |ring: usize, s: usize| -> Pos2 {
        if ring == 0 {
            return origin;
        }
        let k = ring * 31 + s;
        let frac = s as f32 / SECTORS as f32;
        let jitter_a = if s == 0 || s == SECTORS {
            0.0
        } else {
            shash(k, 11) * (0.38 / SECTORS as f32)
        };
        let theta = ((frac + jitter_a) - 0.5) * (PI * 0.86);
        let r_base = reach * ring_fracs[ring];
        let r = r_base * (0.86 + hash(k, 17) * 0.28);
        let dir = normal * theta.cos() + tangent * theta.sin();
        origin + dir * r
    };

    // -------------------------------------------------------------------------
    // 1. Impact seam flare & expanding shockwave (t = 0.0 .. 0.42)
    // -------------------------------------------------------------------------
    if t < 0.42 {
        let flash_t = (t / 0.42).clamp(0.0, 1.0);
        let flash_a = (1.0 - flash_t).powi(2);

        // Bright flare along the crossed screen edge.
        let seam_half = reach * (0.25 + 0.55 * ease_out_cubic(flash_t));
        let p0 = origin - tangent * seam_half;
        let p1 = origin + tangent * seam_half;
        shapes.push(Shape::line_segment(
            [p0, p1],
            Stroke::new(
                6.0 * scale,
                Color32::from_rgba_unmultiplied(70, 210, 255, u8_alpha(0.38 * flash_a)),
            ),
        ));
        shapes.push(Shape::line_segment(
            [p0, p1],
            Stroke::new(
                2.2 * scale,
                Color32::from_rgba_unmultiplied(235, 252, 255, u8_alpha(0.92 * flash_a)),
            ),
        ));

        // Expanding shockwave arc / ring.
        let wave_r = reach * (0.12 + 0.78 * ease_out_cubic(flash_t));
        let wave_center = origin + normal * (wave_r * 0.35);
        shapes.push(Shape::circle_stroke(
            wave_center,
            wave_r * 0.65,
            Stroke::new(
                1.8 * scale * (1.0 - flash_t * 0.5),
                Color32::from_rgba_unmultiplied(150, 235, 255, u8_alpha(0.55 * flash_a)),
            ),
        ));

        // Layered impact glow at the crossing point.
        shapes.push(Shape::circle_filled(
            origin + normal * (4.0 * scale),
            26.0 * scale * (0.6 + 0.5 * flash_t),
            Color32::from_rgba_unmultiplied(80, 215, 255, u8_alpha(0.28 * flash_a)),
        ));
        shapes.push(Shape::circle_filled(
            origin + normal * (2.0 * scale),
            12.0 * scale * (1.0 - 0.3 * flash_t),
            Color32::from_rgba_unmultiplied(255, 255, 255, u8_alpha(0.95 * flash_a)),
        ));
    }

    // -------------------------------------------------------------------------
    // 2. Jagged spiderweb fracture lines (t = 0.0 .. 0.52)
    // -------------------------------------------------------------------------
    if t < 0.52 {
        let grow = ease_out_cubic(t / 0.11);
        let fade = if t < 0.16 {
            1.0
        } else {
            (1.0 - (t - 0.16) / 0.36).clamp(0.0, 1.0).powi(2)
        };
        let glow_stroke = Stroke::new(
            3.2 * scale,
            Color32::from_rgba_unmultiplied(75, 210, 255, u8_alpha(0.34 * fade)),
        );
        let core_stroke = Stroke::new(
            1.25 * scale,
            Color32::from_rgba_unmultiplied(240, 252, 255, u8_alpha(0.92 * fade)),
        );
        let web_stroke = Stroke::new(
            0.95 * scale,
            Color32::from_rgba_unmultiplied(195, 240, 255, u8_alpha(0.68 * fade)),
        );

        let max_ring = ((grow * RINGS as f32).ceil() as usize).clamp(1, RINGS);
        for s in 0..=SECTORS {
            for r in 0..max_ring {
                let a = node(r, s);
                let b_full = node(r + 1, s);
                let seg_prog = ((grow * RINGS as f32) - r as f32).clamp(0.0, 1.0);
                let b = a + (b_full - a) * seg_prog;

                // Midpoint kink for authentic jagged glass fracture look.
                let dir = b - a;
                let perp = vec2(-dir.y, dir.x).normalized();
                let kink = (a + b.to_vec2()) * 0.5
                    + perp * (shash(r * 19 + s, 23) * 4.5 * scale * seg_prog);

                shapes.push(Shape::line_segment([a, kink], glow_stroke));
                shapes.push(Shape::line_segment([kink, b], glow_stroke));
                shapes.push(Shape::line_segment([a, kink], core_stroke));
                shapes.push(Shape::line_segment([kink, b], core_stroke));

                // Cross-webbing between adjacent rays.
                if s < SECTORS && seg_prog > 0.65 {
                    let next = node(r + 1, s + 1);
                    shapes.push(Shape::line_segment([b_full, next], web_stroke));
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // 3. 3D-tumbling crystalline triangle shards (t = 0.04 .. 1.0)
    // -------------------------------------------------------------------------
    if t > 0.04 {
        let gravity = vec2(0.0, reach * 1.55);
        for ring in 0..RINGS {
            for s in 0..SECTORS {
                // Split each cell into 1 (inner ring) or 2 (outer rings) strictly convex triangles.
                let tris: [([Pos2; 3], usize); 2] = if ring == 0 {
                    let tri = [node(0, s), node(1, s), node(1, s + 1)];
                    [(tri, 0), (tri, 1)]
                } else {
                    let p00 = node(ring, s);
                    let p01 = node(ring, s + 1);
                    let p10 = node(ring + 1, s);
                    let p11 = node(ring + 1, s + 1);
                    [([p00, p10, p11], 0), ([p00, p11, p01], 1)]
                };
                let count = if ring == 0 { 1 } else { 2 };

                for (tri, sub) in tris.into_iter().take(count) {
                    let id = ring * 41 + s * 3 + sub;
                    // Inner shards break loose first; outer shards peel off right after.
                    let delay = 0.04 + (ring as f32) * 0.035 + hash(id, 31) * 0.06;
                    if t <= delay {
                        continue;
                    }
                    let f = ((t - delay) / (1.0 - delay)).clamp(0.0, 1.0);
                    let life = (1.0 - f).powf(1.35);
                    if life <= 0.02 {
                        continue;
                    }

                    let center = Pos2::new(
                        (tri[0].x + tri[1].x + tri[2].x) / 3.0,
                        (tri[0].y + tri[1].y + tri[2].y) / 3.0,
                    );
                    let radial = (center - origin).normalized();
                    let speed = reach * (0.42 + hash(id, 37) * 0.65);
                    let lift = vec2(shash(id, 41) * 28.0 * scale, -24.0 * scale * hash(id, 43));
                    let vel = (normal * 0.55 + radial * 0.45) * speed + lift;
                    let offset = vel * (f * (1.0 - 0.25 * f)) + gravity * (f * f);

                    // 2D rotation + faux-3D tumble around a tilted axis.
                    let spin = shash(id, 53) * 4.2 * f;
                    let (sin_r, cos_r) = spin.sin_cos();
                    let tumble_phase = hash(id, 59) * PI + f * (5.5 + hash(id, 61) * 4.5);
                    let tumble = tumble_phase.cos();
                    let squash = tumble.abs().max(0.22);
                    let shrink = 1.0 - 0.22 * f;

                    let moved_center = center + offset;
                    let transform = |p: Pos2| -> Pos2 {
                        let d = (p - center) * shrink;
                        let rx = d.x * cos_r - d.y * sin_r;
                        let ry = (d.x * sin_r + d.y * cos_r) * squash;
                        moved_center + vec2(rx, ry)
                    };
                    let pts = vec![transform(tri[0]), transform(tri[1]), transform(tri[2])];

                    // Prismatic ice palette + specular flash when catching light.
                    let glint = ((tumble.abs() - 0.72) / 0.28).clamp(0.0, 1.0);
                    let (base_r, base_g, base_b) = match id % 3 {
                        0 => (135.0, 225.0, 255.0), // Icy cyan
                        1 => (175.0, 210.0, 255.0), // Crystal periwinkle
                        _ => (150.0, 245.0, 240.0), // Frosted aqua
                    };
                    let r_col = (base_r + (255.0 - base_r) * glint) as u8;
                    let g_col = (base_g + (255.0 - base_g) * glint) as u8;
                    let b_col = (base_b + (255.0 - base_b) * glint) as u8;

                    let fill_alpha = (0.30 + 0.38 * glint) * life;
                    let edge_alpha = (0.78 + 0.22 * glint) * life;
                    let fill =
                        Color32::from_rgba_unmultiplied(r_col, g_col, b_col, u8_alpha(fill_alpha));
                    let edge = Stroke::new(
                        (1.15 + 0.5 * glint) * scale,
                        Color32::from_rgba_unmultiplied(245, 253, 255, u8_alpha(edge_alpha)),
                    );
                    let p0 = pts[0];
                    let p1 = pts[1];
                    let p2 = pts[2];
                    shapes.push(Shape::convex_polygon(pts, fill, edge));

                    // Specular facet reflection line across the shard interior.
                    let facet_a = (p0 + p1.to_vec2()) * 0.5;
                    let facet_b = (p2 + moved_center.to_vec2()) * 0.5;
                    shapes.push(Shape::line_segment(
                        [facet_a, facet_b],
                        Stroke::new(
                            0.9 * scale,
                            Color32::from_rgba_unmultiplied(
                                255,
                                255,
                                255,
                                u8_alpha((0.35 + 0.55 * glint) * life),
                            ),
                        ),
                    ));
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // 4. Cute twinkling 4-point star sparkles (`✦`) (t = 0.03 .. 0.90)
    // -------------------------------------------------------------------------
    if t > 0.03 && t < 0.92 {
        let sp_t = ((t - 0.03) / 0.89).clamp(0.0, 1.0);
        for i in 0..SPARKLES {
            let delay = hash(i, 71) * 0.14;
            if sp_t <= delay {
                continue;
            }
            let f = ((sp_t - delay) / (1.0 - delay)).clamp(0.0, 1.0);
            let fade = (1.0 - f).powf(1.5);
            let theta = shash(i, 73) * (FRAC_PI_2 * 0.92);
            let dir = normal * theta.cos() + tangent * theta.sin();
            let dist = reach * (0.22 + 0.95 * ease_out_cubic(f) * (0.55 + 0.65 * hash(i, 79)));
            let drift_y = f * f * reach * 0.35;
            let pos = origin + dir * dist + vec2(0.0, drift_y);

            let twinkle = (f * 14.0 + hash(i, 83) * 6.0).sin() * 0.5 + 0.5;
            let arm = (3.2 + 3.8 * hash(i, 89)) * scale * (0.55 + 0.65 * twinkle) * fade;
            if arm < 0.6 {
                continue;
            }
            let alpha = u8_alpha((0.55 + 0.45 * twinkle) * fade);
            let star_stroke = Stroke::new(
                1.25 * scale,
                Color32::from_rgba_unmultiplied(245, 254, 255, alpha),
            );
            shapes.push(Shape::line_segment(
                [pos - vec2(arm, 0.0), pos + vec2(arm, 0.0)],
                star_stroke,
            ));
            shapes.push(Shape::line_segment(
                [pos - vec2(0.0, arm), pos + vec2(0.0, arm)],
                star_stroke,
            ));
            shapes.push(Shape::circle_filled(
                pos,
                1.6 * scale * fade,
                Color32::from_rgba_unmultiplied(165, 240, 255, alpha),
            ));
        }
    }
}

/// Where on the screen the glass breaks: the middle of the shared edge, shifted by alignment.
pub fn origin(screen: Rect, side: Side, alignment: f64) -> Pos2 {
    let along = (0.5 + alignment as f32 * 0.5).clamp(0.08, 0.92);
    match side {
        Side::Left => pos2(screen.left(), screen.top() + screen.height() * along),
        Side::Right => pos2(screen.right(), screen.top() + screen.height() * along),
        Side::Top => pos2(screen.left() + screen.width() * along, screen.top()),
        Side::Bottom => pos2(screen.left() + screen.width() * along, screen.bottom()),
    }
}

/// Screen-space rectangle for the compact overlay window anchored around the impact point.
pub fn overlay_rect(monitor: Rect, side: Side, alignment: f64) -> Rect {
    let hit = origin(monitor, side, alignment);
    let half = OVERLAY_SIZE * 0.5;
    let center = match side {
        Side::Left => pos2(monitor.left() + half, hit.y),
        Side::Right => pos2(monitor.right() - half, hit.y),
        Side::Top => pos2(hit.x, monitor.top() + half),
        Side::Bottom => pos2(hit.x, monitor.bottom() - half),
    };
    Rect::from_center_size(center, Vec2::splat(OVERLAY_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_is_bounded_and_renders_across_all_phases() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(460., 460.));
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            for t in [0.0, 0.08, 0.2, 0.5, 0.85, 1.0] {
                let mut shapes = Vec::new();
                paint(&mut shapes, screen, origin(screen, side, 0.0), side, t);
                assert!(shapes.len() <= 350, "too many shapes: {}", shapes.len());
                if t < 0.85 {
                    assert!(!shapes.is_empty(), "expected shapes at t={t}");
                }
            }
        }
    }

    #[test]
    fn origin_sits_on_the_requested_edge() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(100., 50.));
        assert_eq!(origin(screen, Side::Left, 0.).x, 0.);
        assert_eq!(origin(screen, Side::Right, 0.).x, 100.);
        assert_eq!(origin(screen, Side::Top, 0.).y, 0.);
        assert_eq!(origin(screen, Side::Bottom, 0.).y, 50.);
    }
}
