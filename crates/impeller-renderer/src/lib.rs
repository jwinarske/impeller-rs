//! Turning paths into geometry a backend can draw.
//!
//! This is where the two halves meet: tessellation produces triangles in path
//! space, and a backend wants them in clip space. Sitting between them means
//! this layer owns the coordinate mapping and the tolerance scaling that
//! depends on it.

use glam::{Affine2, Vec2};
use impeller_geometry::stroke::StrokeStyle;
use impeller_geometry::tessellate::{Tessellator, VertexBuffers};
use impeller_geometry::transform::{max_scale, transform_points, viewport_projection};
use impeller_geometry::{flatten::DEFAULT_TOLERANCE, Path};
use impeller_hal::Extent2D;

/// Tessellates paths and places the result in clip space.
///
/// Holds its scratch buffers across calls, so a steady frame loop stops
/// allocating once they reach working size.
#[derive(Default)]
pub struct Renderer {
    tessellator: Tessellator,
    clip_space: Vec<Vec2>,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tessellate a filled path and map it into clip space.
    ///
    /// `transform` maps path space to **device pixels** — it does not include
    /// the projection onto clip space, which is derived from `target`.
    /// Separating them is what keeps tolerance correct: tolerance is a
    /// device-space quantity, so the scale that matters for flattening is the
    /// path-to-pixel scale alone. Folding the projection in would divide by
    /// the target size and flatten far too coarsely.
    pub fn fill_path(
        &mut self,
        path: &Path,
        transform: Affine2,
        target: Extent2D,
        tolerance: f32,
    ) -> ClipGeometry<'_> {
        let path_tolerance = path_space_tolerance(tolerance, &transform);
        let buffers = self.tessellator.fill(path, path_tolerance);
        Self::to_clip_space(&mut self.clip_space, buffers, transform, target)
    }

    /// Tessellate a stroked path and map it into clip space.
    ///
    /// The stroke width is in path space, so it is scaled by `transform` along
    /// with everything else. A caller wanting a width that is constant in
    /// device pixels divides it out first.
    pub fn stroke_path(
        &mut self,
        path: &Path,
        style: &StrokeStyle,
        transform: Affine2,
        target: Extent2D,
        tolerance: f32,
    ) -> ClipGeometry<'_> {
        let path_tolerance = path_space_tolerance(tolerance, &transform);
        let buffers = self.tessellator.stroke(path, style, path_tolerance);
        Self::to_clip_space(&mut self.clip_space, buffers, transform, target)
    }

    fn to_clip_space<'a>(
        scratch: &'a mut Vec<Vec2>,
        buffers: &'a VertexBuffers,
        transform: Affine2,
        target: Extent2D,
    ) -> ClipGeometry<'a> {
        // One combined transform rather than two passes: composing first means
        // each vertex is touched once and rounds once.
        let to_clip = viewport_projection(target.width, target.height) * transform;
        scratch.clear();
        scratch.extend_from_slice(&buffers.vertices);
        transform_points(scratch, &to_clip);
        ClipGeometry {
            vertices: scratch,
            indices: &buffers.indices,
        }
    }
}

/// Triangles in clip space, ready for a backend.
pub struct ClipGeometry<'a> {
    pub vertices: &'a [Vec2],
    pub indices: &'a [u32],
}

impl ClipGeometry<'_> {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Vertices as a flat array, which is the shape backends take them in.
    pub fn positions(&self) -> Vec<[f32; 2]> {
        self.vertices.iter().map(|v| [v.x, v.y]).collect()
    }
}

/// Convert a device-space tolerance into path space.
fn path_space_tolerance(tolerance: f32, transform: &Affine2) -> f32 {
    impeller_geometry::flatten::tolerance_for_scale(tolerance, max_scale(transform))
}

/// The default flattening tolerance, in device pixels.
pub const TOLERANCE: f32 = DEFAULT_TOLERANCE;

#[cfg(test)]
mod tests {
    use super::*;
    use impeller_geometry::PathBuilder;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(x0, y0))
            .line_to(Vec2::new(x1, y0))
            .line_to(Vec2::new(x1, y1))
            .line_to(Vec2::new(x0, y1))
            .close();
        b.build()
    }

    #[test]
    fn a_full_target_rect_reaches_the_corners_of_clip_space() {
        let mut r = Renderer::new();
        let geo = r.fill_path(
            &rect(0.0, 0.0, 64.0, 64.0),
            Affine2::IDENTITY,
            Extent2D::new(64, 64),
            TOLERANCE,
        );
        let xs: Vec<f32> = geo.vertices.iter().map(|v| v.x).collect();
        let ys: Vec<f32> = geo.vertices.iter().map(|v| v.y).collect();
        let min = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
        let max = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        assert!((min(&xs) + 1.0).abs() < 1e-5);
        assert!((max(&xs) - 1.0).abs() < 1e-5);
        assert!((min(&ys) + 1.0).abs() < 1e-5);
        assert!((max(&ys) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn tolerance_is_scaled_by_the_transform_but_not_by_the_target_size() {
        // A curve tessellated under a 10x transform must gain segments,
        // because it covers ten times the device pixels.
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .cubic_to(
                Vec2::new(0.0, 10.0),
                Vec2::new(10.0, 10.0),
                Vec2::new(10.0, 0.0),
            )
            .close();
        let path = b.build();
        let target = Extent2D::new(256, 256);

        let mut r = Renderer::new();
        let plain = r
            .fill_path(&path, Affine2::IDENTITY, target, TOLERANCE)
            .triangle_count();
        let scaled = r
            .fill_path(
                &path,
                Affine2::from_scale(Vec2::splat(10.0)),
                target,
                TOLERANCE,
            )
            .triangle_count();
        assert!(
            scaled > plain,
            "scaling up should refine flattening: {plain} then {scaled}"
        );

        // Target size must not affect it. Folding the projection into the
        // tolerance scale would make a large target flatten coarsely.
        let big_target = r
            .fill_path(
                &path,
                Affine2::IDENTITY,
                Extent2D::new(4096, 4096),
                TOLERANCE,
            )
            .triangle_count();
        assert_eq!(big_target, plain);
    }

    #[test]
    fn an_empty_path_produces_no_geometry() {
        let mut r = Renderer::new();
        let geo = r.fill_path(
            &Path::default(),
            Affine2::IDENTITY,
            Extent2D::new(64, 64),
            TOLERANCE,
        );
        assert!(geo.is_empty());
        assert!(geo.positions().is_empty());
    }
}
