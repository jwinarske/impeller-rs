//! Path representation.
//!
//! Paths are stored as a verb stream with a parallel point buffer rather than
//! as an enum-per-segment vector. Tessellation walks both linearly, and
//! keeping points contiguous means the walk touches one cache line per few
//! segments instead of chasing a pointer per segment.

use glam::Vec2;

/// How overlapping subpaths combine when filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    /// A point is inside when the signed crossing count is nonzero.
    #[default]
    NonZero,
    /// A point is inside when the crossing count is odd.
    EvenOdd,
}

/// A path segment operator.
///
/// Each verb consumes a fixed number of points from the point buffer, given by
/// [`Verb::point_count`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    MoveTo,
    LineTo,
    QuadTo,
    CubicTo,
    Close,
}

impl Verb {
    /// Points this verb consumes from the point buffer.
    pub const fn point_count(self) -> usize {
        match self {
            Self::MoveTo | Self::LineTo => 1,
            Self::QuadTo => 2,
            Self::CubicTo => 3,
            Self::Close => 0,
        }
    }
}

/// An axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// The empty rect, which absorbs any point when unioned.
    pub fn empty() -> Self {
        Self {
            min: Vec2::splat(f32::INFINITY),
            max: Vec2::splat(f32::NEG_INFINITY),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    pub fn union_point(&mut self, p: Vec2) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn width(&self) -> f32 {
        (self.max.x - self.min.x).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.max.y - self.min.y).max(0.0)
    }
}

/// Whether a subpath is convex, which selects the fill strategy.
///
/// Convex paths take a fan-fill fast path that skips both tessellation and the
/// stencil pass, so detection is worth doing — but a wrong `Convex` answer
/// renders incorrectly, while a wrong `Concave` answer only costs speed.
/// Detection is therefore conservative: anything it cannot prove convex is
/// reported [`Convexity::Concave`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convexity {
    Convex,
    Concave,
}

/// A path: a verb stream plus the points those verbs consume.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    verbs: Vec<Verb>,
    points: Vec<Vec2>,
    fill_rule: FillRule,
}

impl Path {
    pub fn builder() -> PathBuilder {
        PathBuilder::new()
    }

    pub fn verbs(&self) -> &[Verb] {
        &self.verbs
    }

    pub fn points(&self) -> &[Vec2] {
        &self.points
    }

    pub fn fill_rule(&self) -> FillRule {
        self.fill_rule
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// Bounds of the control points.
    ///
    /// This is a bound, not a tight fit: a curve lies within the convex hull of
    /// its control points, so a curve that bends away from its handles reports
    /// a larger box than it occupies. That is the right trade for culling,
    /// where a conservative overestimate is safe and cheap while a tight fit
    /// costs a solve per curve.
    pub fn bounds(&self) -> Rect {
        let mut r = Rect::empty();
        for p in &self.points {
            r.union_point(*p);
        }
        r
    }

    /// Walk the path as `(verb, points)` pairs.
    pub fn segments(&self) -> impl Iterator<Item = (Verb, &[Vec2])> {
        let mut cursor = 0usize;
        self.verbs.iter().map(move |verb| {
            let n = verb.point_count();
            let slice = &self.points[cursor..cursor + n];
            cursor += n;
            (*verb, slice)
        })
    }

    /// Whether the path is provably convex.
    ///
    /// Answers [`Convexity::Concave`] for anything with more than one subpath,
    /// or containing curves, rather than analyzing further. Curved paths are
    /// classified after flattening, where the question is a polygon question.
    pub fn convexity(&self) -> Convexity {
        if self
            .verbs
            .iter()
            .any(|v| matches!(v, Verb::QuadTo | Verb::CubicTo))
        {
            return Convexity::Concave;
        }
        if self.verbs.iter().filter(|v| **v == Verb::MoveTo).count() > 1 {
            return Convexity::Concave;
        }
        polygon_convexity(&self.points)
    }
}

/// Which quadrant a direction points into, as a number that increases
/// counter-clockwise.
///
/// Zero-length directions land in the first quadrant, which is harmless: a
/// repeated point contributes no rotation and the count below is of net
/// advance, not of steps.
fn quadrant(v: Vec2) -> i32 {
    match (v.x >= 0.0, v.y >= 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    }
}

/// Classify a polygon by the consistency of its turn directions.
///
/// A simple polygon is convex when every turn goes the same way. Collinear
/// points contribute a zero cross product and are skipped rather than treated
/// as a reversal, since they are common in generated geometry and do not
/// affect convexity.
///
/// Turn agreement alone is not enough, because a self-crossing polygon can
/// have every turn agree: a pentagram turns the same way at all five points
/// and is not remotely convex. What separates them is how far the polygon
/// turns in total. Walking a simple closed polygon comes back to the start
/// having turned through exactly one full circle; walking a pentagram turns
/// through two.
///
/// That total is counted rather than measured. Once the turns are known to
/// agree the direction rotates monotonically, so each step advances through
/// zero, one or two quadrants in the one direction and never doubles back --
/// and summing those advances gives four per revolution exactly. The
/// alternative, accumulating the turn angles with `atan2`, computes the same
/// number and measured six times slower than the whole rest of this function,
/// on a path that runs once per filled path per frame.
///
/// This matters well beyond classification. The caller uses `Convex` to take a
/// fan fill, which triangulates from one vertex and applies no fill rule at
/// all -- so a self-crossing polygon called convex is filled by a routine that
/// cannot express what filling it means, and its fill rule is silently
/// discarded.
pub fn polygon_convexity(points: &[Vec2]) -> Convexity {
    if points.len() < 3 {
        return Convexity::Convex;
    }
    let n = points.len();
    let mut sign = 0i32;
    // Both readings of the advance, since which one counts depends on the
    // direction of travel and that is not settled until the walk finishes.
    // Two running sums rather than a list of steps, so this allocates nothing.
    let (mut counter_clockwise, mut clockwise) = (0i32, 0i32);
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let c = points[(i + 2) % n];
        let incoming = b - a;
        let outgoing = c - b;
        let cross = incoming.perp_dot(outgoing);
        let advance = (quadrant(outgoing) - quadrant(incoming)).rem_euclid(4);
        counter_clockwise += advance;
        clockwise += (-advance).rem_euclid(4);
        if cross.abs() <= f32::EPSILON {
            continue;
        }
        let s = if cross > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return Convexity::Concave;
        }
    }
    if sign == 0 {
        // Every turn was collinear, so the polygon is a segment traversed out
        // and back. Degenerate rather than self-crossing, and it covers no
        // area either way.
        return Convexity::Convex;
    }
    let turned = if sign > 0 {
        counter_clockwise
    } else {
        clockwise
    };
    if turned > 4 {
        return Convexity::Concave;
    }
    Convexity::Convex
}

/// Accumulates verbs and points into a [`Path`].
#[derive(Debug, Clone, Default)]
pub struct PathBuilder {
    verbs: Vec<Verb>,
    points: Vec<Vec2>,
    fill_rule: FillRule,
    /// Where the current subpath started, for `close`.
    subpath_start: Option<Vec2>,
    current: Option<Vec2>,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fill_rule(mut self, rule: FillRule) -> Self {
        self.fill_rule = rule;
        self
    }

    pub fn move_to(&mut self, p: Vec2) -> &mut Self {
        self.verbs.push(Verb::MoveTo);
        self.points.push(p);
        self.subpath_start = Some(p);
        self.current = Some(p);
        self
    }

    /// Add a line segment.
    ///
    /// A line before any `move_to` implicitly starts the subpath at the
    /// origin, matching how path data from SVG and font outlines behaves when
    /// it omits the opening move.
    pub fn line_to(&mut self, p: Vec2) -> &mut Self {
        self.ensure_started();
        self.verbs.push(Verb::LineTo);
        self.points.push(p);
        self.current = Some(p);
        self
    }

    pub fn quad_to(&mut self, ctrl: Vec2, to: Vec2) -> &mut Self {
        self.ensure_started();
        self.verbs.push(Verb::QuadTo);
        self.points.push(ctrl);
        self.points.push(to);
        self.current = Some(to);
        self
    }

    pub fn cubic_to(&mut self, c0: Vec2, c1: Vec2, to: Vec2) -> &mut Self {
        self.ensure_started();
        self.verbs.push(Verb::CubicTo);
        self.points.push(c0);
        self.points.push(c1);
        self.points.push(to);
        self.current = Some(to);
        self
    }

    /// Close the current subpath.
    ///
    /// Closing an empty builder is a no-op rather than an error, so callers
    /// replaying arbitrary path data do not have to special-case it.
    pub fn close(&mut self) -> &mut Self {
        if self.verbs.is_empty() {
            return self;
        }
        self.verbs.push(Verb::Close);
        self.current = self.subpath_start;
        self
    }

    /// Current pen position, if the path has started.
    pub fn current_point(&self) -> Option<Vec2> {
        self.current
    }

    fn ensure_started(&mut self) {
        if self.verbs.is_empty() {
            self.verbs.push(Verb::MoveTo);
            self.points.push(Vec2::ZERO);
            self.subpath_start = Some(Vec2::ZERO);
            self.current = Some(Vec2::ZERO);
        }
    }

    pub fn build(self) -> Path {
        Path {
            verbs: self.verbs,
            points: self.points,
            fill_rule: self.fill_rule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_point_counts_match_what_the_builder_pushes() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(1.0, 0.0))
            .quad_to(Vec2::new(2.0, 1.0), Vec2::new(3.0, 0.0))
            .cubic_to(
                Vec2::new(4.0, 1.0),
                Vec2::new(5.0, -1.0),
                Vec2::new(6.0, 0.0),
            )
            .close();
        let path = b.build();

        let expected: usize = path.verbs().iter().map(|v| v.point_count()).sum();
        // A mismatch here would desynchronize the parallel buffers and make
        // every later segment read the wrong points.
        assert_eq!(expected, path.points().len());
    }

    #[test]
    fn segments_walk_verbs_and_points_in_step() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .quad_to(Vec2::new(1.0, 1.0), Vec2::new(2.0, 0.0));
        let path = b.build();

        let collected: Vec<_> = path.segments().collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].0, Verb::MoveTo);
        assert_eq!(collected[0].1, &[Vec2::ZERO]);
        assert_eq!(collected[1].0, Verb::QuadTo);
        assert_eq!(collected[1].1, &[Vec2::new(1.0, 1.0), Vec2::new(2.0, 0.0)]);
    }

    #[test]
    fn line_without_move_starts_at_origin() {
        let mut b = PathBuilder::new();
        b.line_to(Vec2::new(1.0, 1.0));
        let path = b.build();
        assert_eq!(path.verbs()[0], Verb::MoveTo);
        assert_eq!(path.points()[0], Vec2::ZERO);
    }

    #[test]
    fn closing_an_empty_builder_is_a_no_op() {
        let path = {
            let mut b = PathBuilder::new();
            b.close();
            b.build()
        };
        assert!(path.is_empty());
    }

    #[test]
    fn close_returns_the_pen_to_the_subpath_start() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(5.0, 5.0))
            .line_to(Vec2::new(9.0, 5.0))
            .close();
        assert_eq!(b.current_point(), Some(Vec2::new(5.0, 5.0)));
    }

    #[test]
    fn bounds_cover_every_control_point() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0)).cubic_to(
            Vec2::new(0.0, 10.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(10.0, 0.0),
        );
        let path = b.build();
        let bounds = path.bounds();
        for p in path.points() {
            assert!(bounds.contains(*p), "bounds must contain {p:?}");
        }
        // Control-point hull, not a tight fit: the curve never reaches y=10.
        assert_eq!(bounds.max.y, 10.0);
    }

    #[test]
    fn empty_path_has_empty_bounds() {
        let path = Path::default();
        assert!(path.bounds().is_empty());
        assert_eq!(path.bounds().width(), 0.0);
    }

    #[test]
    fn square_is_convex_and_chevron_is_not() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        assert_eq!(polygon_convexity(&square), Convexity::Convex);

        // A chevron reverses turn direction at the notch.
        let chevron = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(2.0, 4.0),
        ];
        assert_eq!(polygon_convexity(&chevron), Convexity::Concave);
    }

    #[test]
    fn collinear_points_do_not_break_convexity() {
        // Generated geometry frequently contains collinear runs; treating a
        // zero cross product as a reversal would reject valid fan-fill cases.
        let with_collinear = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        assert_eq!(polygon_convexity(&with_collinear), Convexity::Convex);
    }

    #[test]
    fn convexity_is_conservative_about_curves_and_subpaths() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .quad_to(Vec2::new(1.0, 1.0), Vec2::new(2.0, 0.0));
        // Curves are classified after flattening, so the unflattened answer
        // errs toward the slow path rather than risking a wrong fast path.
        assert_eq!(b.build().convexity(), Convexity::Concave);

        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(1.0, 0.0))
            .close()
            .move_to(Vec2::new(5.0, 5.0))
            .line_to(Vec2::new(6.0, 5.0));
        assert_eq!(b.build().convexity(), Convexity::Concave);
    }

    #[test]
    fn degenerate_polygons_are_trivially_convex() {
        assert_eq!(polygon_convexity(&[]), Convexity::Convex);
        assert_eq!(polygon_convexity(&[Vec2::ZERO]), Convexity::Convex);
        assert_eq!(
            polygon_convexity(&[Vec2::ZERO, Vec2::new(1.0, 1.0)]),
            Convexity::Convex
        );
    }
}
