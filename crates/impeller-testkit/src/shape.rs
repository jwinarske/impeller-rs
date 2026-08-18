//! Shapes as data.
//!
//! Scenes describe geometry declaratively rather than by calling a builder, so
//! one corpus can drive golden comparison, cross-backend conformance,
//! performance runs, and on-device runs without being rewritten for each. Every
//! type here is plain data and can be serialized when the corpus moves out of
//! Rust source.

use glam::Vec2;
use impeller_geometry::{FillRule, Path, PathBuilder};

/// The magic constant for approximating a quarter circle with a cubic.
const KAPPA: f32 = 0.552_284_8;

/// A shape, in user coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect {
        min: [f32; 2],
        max: [f32; 2],
    },
    /// A closed polygon through the given points.
    Polygon(Vec<[f32; 2]>),
    /// An open run of line segments.
    ///
    /// Distinct from [`Self::Polygon`] in the one way that matters to a
    /// stroke: an open path has two ends, so it has caps. A closed one has a
    /// join everywhere instead, which is why a corpus of closed shapes could
    /// not exercise a cap however many strokes it contained.
    Polyline(Vec<[f32; 2]>),
    /// A closed polygon filled by a stated rule.
    ///
    /// Separate from [`Self::Polygon`] rather than a field on it, because the
    /// rule only ever differs from the default for a path that crosses itself
    /// -- every other polygon fills the same either way -- and putting it on
    /// the common variant would make each of the corpus's many polygons state
    /// something that does not matter to it.
    RuledPolygon {
        points: Vec<[f32; 2]>,
        rule: FillRule,
    },
    Circle {
        center: [f32; 2],
        radius: f32,
    },
    /// An ellipse inscribed in a rectangle.
    ///
    /// Not expressible as any of the others: a rounded rectangle asked for a
    /// large radius becomes a stadium rather than an ellipse, and a circle
    /// scaled by a transform is one only if the whole item is scaled with it.
    Oval {
        min: [f32; 2],
        max: [f32; 2],
    },
    /// A rectangle with rounded corners, which is most of an interface.
    RoundedRect {
        min: [f32; 2],
        max: [f32; 2],
        /// Clamped to half the shorter side, so a large one gives a stadium
        /// rather than an outline that crosses itself.
        radius: f32,
    },
    /// An open cubic, for curve and stroke coverage.
    Cubic {
        start: [f32; 2],
        c0: [f32; 2],
        c1: [f32; 2],
        end: [f32; 2],
    },
}

impl Shape {
    pub fn to_path(&self) -> Path {
        let mut b = PathBuilder::new();
        match self {
            Self::Rect { min, max } => {
                b.move_to(Vec2::from(*min))
                    .line_to(Vec2::new(max[0], min[1]))
                    .line_to(Vec2::from(*max))
                    .line_to(Vec2::new(min[0], max[1]))
                    .close();
            }
            Self::Polygon(points) => {
                trace(&mut b, points);
                b.close();
            }
            Self::Polyline(points) => {
                trace(&mut b, points);
            }
            Self::RuledPolygon { points, rule } => {
                b = b.with_fill_rule(*rule);
                trace(&mut b, points);
                b.close();
            }
            Self::Oval { min, max } => {
                let (cx, cy) = ((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0);
                let (rx, ry) = ((max[0] - min[0]) / 2.0, (max[1] - min[1]) / 2.0);
                let (kx, ky) = (KAPPA * rx, KAPPA * ry);
                b.move_to(Vec2::new(cx + rx, cy))
                    .cubic_to(
                        Vec2::new(cx + rx, cy + ky),
                        Vec2::new(cx + kx, cy + ry),
                        Vec2::new(cx, cy + ry),
                    )
                    .cubic_to(
                        Vec2::new(cx - kx, cy + ry),
                        Vec2::new(cx - rx, cy + ky),
                        Vec2::new(cx - rx, cy),
                    )
                    .cubic_to(
                        Vec2::new(cx - rx, cy - ky),
                        Vec2::new(cx - kx, cy - ry),
                        Vec2::new(cx, cy - ry),
                    )
                    .cubic_to(
                        Vec2::new(cx + kx, cy - ry),
                        Vec2::new(cx + rx, cy - ky),
                        Vec2::new(cx + rx, cy),
                    )
                    .close();
            }
            Self::RoundedRect { min, max, radius } => {
                let (l, t) = (min[0], min[1]);
                let (r, bo) = (max[0], max[1]);
                let radius = radius.min((r - l) / 2.0).min((bo - t) / 2.0).max(0.0);
                let k = KAPPA * radius;
                b.move_to(Vec2::new(l + radius, t))
                    .line_to(Vec2::new(r - radius, t))
                    .cubic_to(
                        Vec2::new(r - radius + k, t),
                        Vec2::new(r, t + radius - k),
                        Vec2::new(r, t + radius),
                    )
                    .line_to(Vec2::new(r, bo - radius))
                    .cubic_to(
                        Vec2::new(r, bo - radius + k),
                        Vec2::new(r - radius + k, bo),
                        Vec2::new(r - radius, bo),
                    )
                    .line_to(Vec2::new(l + radius, bo))
                    .cubic_to(
                        Vec2::new(l + radius - k, bo),
                        Vec2::new(l, bo - radius + k),
                        Vec2::new(l, bo - radius),
                    )
                    .line_to(Vec2::new(l, t + radius))
                    .cubic_to(
                        Vec2::new(l, t + radius - k),
                        Vec2::new(l + radius - k, t),
                        Vec2::new(l + radius, t),
                    )
                    .close();
            }
            Self::Circle { center, radius } => {
                let (cx, cy) = (center[0], center[1]);
                let r = *radius;
                let k = KAPPA * r;
                b.move_to(Vec2::new(cx + r, cy))
                    .cubic_to(
                        Vec2::new(cx + r, cy + k),
                        Vec2::new(cx + k, cy + r),
                        Vec2::new(cx, cy + r),
                    )
                    .cubic_to(
                        Vec2::new(cx - k, cy + r),
                        Vec2::new(cx - r, cy + k),
                        Vec2::new(cx - r, cy),
                    )
                    .cubic_to(
                        Vec2::new(cx - r, cy - k),
                        Vec2::new(cx - k, cy - r),
                        Vec2::new(cx, cy - r),
                    )
                    .cubic_to(
                        Vec2::new(cx + k, cy - r),
                        Vec2::new(cx + r, cy - k),
                        Vec2::new(cx + r, cy),
                    )
                    .close();
            }
            Self::Cubic { start, c0, c1, end } => {
                b.move_to(Vec2::from(*start)).cubic_to(
                    Vec2::from(*c0),
                    Vec2::from(*c1),
                    Vec2::from(*end),
                );
            }
        }
        b.build()
    }
}

/// Run a line through the given points, leaving the path open.
fn trace(b: &mut PathBuilder, points: &[[f32; 2]]) {
    if let Some((first, rest)) = points.split_first() {
        b.move_to(Vec2::from(*first));
        for p in rest {
            b.line_to(Vec2::from(*p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_encloses_its_corners() {
        let shape = Shape::Rect {
            min: [10.0, 20.0],
            max: [30.0, 50.0],
        };
        let bounds = shape.to_path().bounds();
        assert_eq!(bounds.min, Vec2::new(10.0, 20.0));
        assert_eq!(bounds.max, Vec2::new(30.0, 50.0));
    }

    #[test]
    fn a_circle_is_bounded_by_its_radius() {
        let shape = Shape::Circle {
            center: [50.0, 50.0],
            radius: 20.0,
        };
        let bounds = shape.to_path().bounds();
        // Control points of a cubic circle approximation lie within the
        // bounding box of the circle itself on the axes.
        assert!((bounds.min.x - 30.0).abs() < 0.01);
        assert!((bounds.max.x - 70.0).abs() < 0.01);
    }

    #[test]
    fn an_empty_polygon_produces_an_empty_path() {
        // Scenes come from data and may be degenerate; converting must not
        // panic on the way in.
        assert!(Shape::Polygon(Vec::new()).to_path().is_empty());
    }
}
