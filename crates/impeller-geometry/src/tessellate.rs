//! Turning paths into triangles.
//!
//! Two strategies, selected by geometry rather than by caller preference:
//!
//! - **Fan fill** for convex paths. A convex polygon triangulates by fanning
//!   from its first vertex, which needs no sweep, no intermediate structures,
//!   and no stencil pass. Most UI geometry — buttons, cards, indicators — is
//!   convex, so this is the common case rather than an optimization for a rare
//!   one.
//! - **General tessellation** through lyon for everything else.
//!
//! Choosing wrongly is not symmetric. Fanning a concave polygon emits
//! triangles that cover area outside the path, which renders visibly wrong;
//! sending a convex path through the general tessellator merely costs time.
//! Convexity detection is conservative for exactly this reason.

use crate::flatten::flatten;
use crate::path::{polygon_convexity, Convexity, FillRule, Path};
use glam::Vec2;

/// Triangles, as a vertex buffer plus an index buffer.
///
/// Indexed rather than expanded so shared vertices are transformed once, which
/// matters for the fan case where every triangle shares vertex zero.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VertexBuffers {
    pub vertices: Vec<Vec2>,
    pub indices: Vec<u32>,
}

impl VertexBuffers {
    /// Drop the contents but keep the allocations.
    ///
    /// The frame loop tessellates repeatedly; reusing capacity is what keeps
    /// per-frame allocation out of the steady state.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Whether every index addresses a real vertex and the buffer is whole
    /// triangles. Used by tests and debug assertions.
    pub fn is_well_formed(&self) -> bool {
        self.indices.len() % 3 == 0
            && self
                .indices
                .iter()
                .all(|i| (*i as usize) < self.vertices.len())
    }
}

/// Reusable tessellation scratch space.
///
/// Holds the output buffers and lyon's internal state across calls so a steady
/// frame loop stops allocating once buffers reach their working size.
#[derive(Default)]
pub struct Tessellator {
    buffers: VertexBuffers,
    fill: lyon_tessellation::FillTessellator,
}

impl Tessellator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tessellate a filled path, returning triangles in path space.
    ///
    /// `tolerance` is the flattening tolerance in device pixels, already
    /// scaled for the transform the result will be drawn under.
    pub fn fill(&mut self, path: &Path, tolerance: f32) -> &VertexBuffers {
        self.buffers.clear();

        let polylines = flatten(path, tolerance);
        if polylines.is_empty() {
            return &self.buffers;
        }

        // A single convex subpath is the common UI case and needs no sweep.
        if polylines.len() == 1
            && polygon_convexity(strip_closing_duplicate(&polylines[0])) == Convexity::Convex
        {
            fan_fill(strip_closing_duplicate(&polylines[0]), &mut self.buffers);
            return &self.buffers;
        }

        general_fill(
            &polylines,
            path.fill_rule(),
            &mut self.fill,
            &mut self.buffers,
        );
        &self.buffers
    }

    pub fn buffers(&self) -> &VertexBuffers {
        &self.buffers
    }
}

/// Drop a trailing point that merely repeats the first.
///
/// Flattening closes subpaths by appending the start point, which is what the
/// tessellator wants but would make convexity analysis see a zero-length edge.
fn strip_closing_duplicate(points: &[Vec2]) -> &[Vec2] {
    if points.len() > 2 && points.first() == points.last() {
        &points[..points.len() - 1]
    } else {
        points
    }
}

/// Triangulate a convex polygon by fanning from its first vertex.
fn fan_fill(points: &[Vec2], out: &mut VertexBuffers) {
    if points.len() < 3 {
        return;
    }
    out.vertices.extend_from_slice(points);
    for i in 1..points.len() as u32 - 1 {
        out.indices.extend_from_slice(&[0, i, i + 1]);
    }
}

/// Tessellate arbitrary geometry through lyon.
fn general_fill(
    polylines: &[Vec<Vec2>],
    fill_rule: FillRule,
    tessellator: &mut lyon_tessellation::FillTessellator,
    out: &mut VertexBuffers,
) {
    use lyon_tessellation::math::Point as LyonPoint;
    use lyon_tessellation::path::Path as LyonPath;
    use lyon_tessellation::{BuffersBuilder, FillOptions, FillRule as LyonFillRule};

    let mut builder = LyonPath::builder();
    for line in polylines {
        let stripped = strip_closing_duplicate(line);
        if stripped.len() < 3 {
            continue;
        }
        builder.begin(LyonPoint::new(stripped[0].x, stripped[0].y));
        for p in &stripped[1..] {
            builder.line_to(LyonPoint::new(p.x, p.y));
        }
        builder.close();
    }
    let lyon_path = builder.build();

    let options = FillOptions::default().with_fill_rule(match fill_rule {
        FillRule::NonZero => LyonFillRule::NonZero,
        FillRule::EvenOdd => LyonFillRule::EvenOdd,
    });

    // u32 indices, not lyon's u16 default: 65536 vertices is well within reach
    // for a detailed path, and silently truncating there would corrupt
    // geometry rather than merely degrade it.
    let mut geometry: lyon_tessellation::VertexBuffers<Vec2, u32> =
        lyon_tessellation::VertexBuffers::new();
    {
        let mut builder = BuffersBuilder::new(&mut geometry, ToVec2);
        // Tessellation failure means self-intersecting or otherwise
        // pathological input. Emitting nothing is the graceful outcome: a
        // missing shape is recoverable, a panic in the frame loop is not.
        if tessellator
            .tessellate_path(&lyon_path, &options, &mut builder)
            .is_err()
        {
            return;
        }
    }

    out.vertices.extend_from_slice(&geometry.vertices);
    out.indices.extend_from_slice(&geometry.indices);
}

/// Emits lyon's vertices directly as [`Vec2`], avoiding a conversion pass over
/// the buffer after tessellation.
struct ToVec2;

impl lyon_tessellation::FillVertexConstructor<Vec2> for ToVec2 {
    fn new_vertex(&mut self, vertex: lyon_tessellation::FillVertex) -> Vec2 {
        let p = vertex.position();
        Vec2::new(p.x, p.y)
    }
}

/// Twice the signed area of a triangle. Positive is counter-clockwise.
fn signed_area2(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - a)
}

/// Total unsigned area covered by a triangle buffer.
pub fn covered_area(buffers: &VertexBuffers) -> f32 {
    buffers
        .indices
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (
                buffers.vertices[t[0] as usize],
                buffers.vertices[t[1] as usize],
                buffers.vertices[t[2] as usize],
            );
            signed_area2(a, b, c).abs() * 0.5
        })
        .sum()
}

/// Unsigned area of a simple polygon, by the shoelace formula.
pub fn polygon_area(points: &[Vec2]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        acc += a.perp_dot(b);
    }
    (acc * 0.5).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatten::DEFAULT_TOLERANCE;
    use crate::path::PathBuilder;

    fn square() -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 10.0))
            .line_to(Vec2::new(0.0, 10.0))
            .close();
        b.build()
    }

    /// An L shape: concave, so it must not take the fan path.
    fn el_shape() -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 4.0))
            .line_to(Vec2::new(4.0, 4.0))
            .line_to(Vec2::new(4.0, 10.0))
            .line_to(Vec2::new(0.0, 10.0))
            .close();
        b.build()
    }

    #[test]
    fn convex_fill_fans_from_the_first_vertex() {
        let mut t = Tessellator::new();
        let buffers = t.fill(&square(), DEFAULT_TOLERANCE);

        // A fan over n vertices is exactly n-2 triangles, all sharing vertex 0.
        assert_eq!(buffers.vertices.len(), 4);
        assert_eq!(buffers.triangle_count(), 2);
        assert!(buffers.indices.chunks_exact(3).all(|t| t[0] == 0));
        assert!(buffers.is_well_formed());
    }

    #[test]
    fn convex_fill_covers_exactly_the_polygon_area() {
        let mut t = Tessellator::new();
        let buffers = t.fill(&square(), DEFAULT_TOLERANCE);
        assert!((covered_area(buffers) - 100.0).abs() < 0.01);
    }

    #[test]
    fn concave_fill_covers_the_polygon_and_not_its_hull() {
        let path = el_shape();
        let mut t = Tessellator::new();
        let buffers = t.fill(&path, DEFAULT_TOLERANCE);

        assert!(buffers.is_well_formed());
        // The L covers 64 units; its convex hull covers 100. Fanning a concave
        // polygon would paint the notch, so this is the assertion that catches
        // a wrong strategy choice.
        let area = covered_area(buffers);
        assert!(
            (area - 64.0).abs() < 0.5,
            "expected the L's own area, got {area}"
        );
    }

    #[test]
    fn indices_always_address_real_vertices() {
        let mut t = Tessellator::new();
        for path in [square(), el_shape()] {
            let buffers = t.fill(&path, DEFAULT_TOLERANCE);
            assert!(buffers.is_well_formed());
        }
    }

    #[test]
    fn empty_and_degenerate_paths_produce_no_triangles() {
        let mut t = Tessellator::new();
        assert!(t.fill(&Path::default(), DEFAULT_TOLERANCE).is_empty());

        // A subpath with fewer than three distinct points encloses nothing.
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO).line_to(Vec2::new(1.0, 1.0)).close();
        assert!(t.fill(&b.build(), DEFAULT_TOLERANCE).is_empty());
    }

    #[test]
    fn buffers_are_reused_across_calls_without_leaking_previous_geometry() {
        let mut t = Tessellator::new();
        let first = t.fill(&el_shape(), DEFAULT_TOLERANCE).triangle_count();
        assert!(first > 0);

        let second = t.fill(&square(), DEFAULT_TOLERANCE);
        // Stale triangles from the previous path would inflate this.
        assert_eq!(second.triangle_count(), 2);
        assert_eq!(second.vertices.len(), 4);
    }

    #[test]
    fn fill_rule_changes_the_result_for_overlapping_subpaths() {
        // A square with a smaller square inside, wound the same way. Non-zero
        // fills the whole outer square; even-odd leaves the inner one hollow.
        let build = |rule: FillRule| {
            let mut b = PathBuilder::new().with_fill_rule(rule);
            b.move_to(Vec2::new(0.0, 0.0))
                .line_to(Vec2::new(10.0, 0.0))
                .line_to(Vec2::new(10.0, 10.0))
                .line_to(Vec2::new(0.0, 10.0))
                .close()
                .move_to(Vec2::new(3.0, 3.0))
                .line_to(Vec2::new(7.0, 3.0))
                .line_to(Vec2::new(7.0, 7.0))
                .line_to(Vec2::new(3.0, 7.0))
                .close();
            b.build()
        };

        let mut t = Tessellator::new();
        let nonzero = covered_area(t.fill(&build(FillRule::NonZero), DEFAULT_TOLERANCE));
        let evenodd = covered_area(t.fill(&build(FillRule::EvenOdd), DEFAULT_TOLERANCE));

        assert!((nonzero - 100.0).abs() < 0.5, "non-zero got {nonzero}");
        assert!((evenodd - 84.0).abs() < 0.5, "even-odd got {evenodd}");
    }

    #[test]
    fn polygon_area_matches_the_shoelace_result() {
        let unit = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 3.0),
            Vec2::new(0.0, 3.0),
        ];
        assert!((polygon_area(&unit) - 6.0).abs() < 1e-5);
        // Winding direction must not change the magnitude.
        let reversed: Vec<_> = unit.iter().rev().copied().collect();
        assert!((polygon_area(&reversed) - 6.0).abs() < 1e-5);
    }

    #[test]
    fn a_curved_path_tessellates_to_roughly_its_true_area() {
        // A circle approximated by four cubics, radius 10, area ~314.16.
        let r = 10.0f32;
        let k = 0.552_284_8 * r;
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(r, 0.0))
            .cubic_to(Vec2::new(r, k), Vec2::new(k, r), Vec2::new(0.0, r))
            .cubic_to(Vec2::new(-k, r), Vec2::new(-r, k), Vec2::new(-r, 0.0))
            .cubic_to(Vec2::new(-r, -k), Vec2::new(-k, -r), Vec2::new(0.0, -r))
            .cubic_to(Vec2::new(k, -r), Vec2::new(r, -k), Vec2::new(r, 0.0))
            .close();

        let mut t = Tessellator::new();
        let area = covered_area(t.fill(&b.build(), 0.05));
        let expected = std::f32::consts::PI * r * r;
        // Flattening inscribes the curve, so the result is slightly under.
        assert!(
            (area - expected).abs() / expected < 0.01,
            "got {area}, expected about {expected}"
        );
    }
}
