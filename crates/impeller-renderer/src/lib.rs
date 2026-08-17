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
use impeller_hal::{Batch, BlendMode, Extent2D, Material, Result, Scissor, Stop};

/// How a shape is painted.
///
/// Grouped rather than passed as loose parameters because color and blend mode
/// travel together everywhere and will grow into the material set: gradients,
/// image shaders, and filters all attach here.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    /// What fills the shape, already resolved into clip space.
    pub material: Material,
    pub blend: BlendMode,
    /// The region of the target this may write to, in device pixels.
    ///
    /// Per-draw state exactly like the blend mode, and carried here for the
    /// same reason: the clip in force when a shape is recorded is a property of
    /// that shape's draw, not of the batch. `None` is the whole target.
    pub clip: Option<Scissor>,
}

impl Paint {
    pub fn solid(color: [f32; 4]) -> Self {
        Self {
            material: Material::solid(color),
            blend: BlendMode::default(),
            clip: None,
        }
    }

    /// A linear gradient between two points **in user space**.
    ///
    /// The endpoints are transformed alongside the geometry, so a gradient
    /// rotates and scales with the shape it fills rather than staying pinned to
    /// the screen.
    pub fn linear_gradient(
        start: Vec2,
        end: Vec2,
        stops: Vec<Stop>,
        transform: Affine2,
        target: Extent2D,
    ) -> Self {
        let to_clip = viewport_projection(target.width, target.height) * transform;
        let start = to_clip.transform_point2(start);
        let end = to_clip.transform_point2(end);
        Self {
            material: Material::LinearGradient {
                start: [start.x, start.y],
                end: [end.x, end.y],
                stops,
            },
            blend: BlendMode::default(),
            clip: None,
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_clip(mut self, clip: Option<Scissor>) -> Self {
        self.clip = clip;
        self
    }
}

/// Tessellates paths and places the result in clip space.
///
/// Holds its scratch buffers across calls, so a steady frame loop stops
/// allocating once they reach working size.
///
/// Target size and tolerance are frame state rather than per-call arguments:
/// they are fixed for every shape in a frame, and threading them through each
/// call invited the mistake of passing different values within one frame.
pub struct Renderer {
    tessellator: Tessellator,
    clip_space: Vec<Vec2>,
    target: Extent2D,
    tolerance: f32,
}

impl Default for Renderer {
    fn default() -> Self {
        Self {
            tessellator: Tessellator::default(),
            clip_space: Vec::new(),
            target: Extent2D::new(1, 1),
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the target size and flattening tolerance for the frame.
    ///
    /// Must be called before drawing; the default target is a single pixel, so
    /// forgetting produces geometry collapsed to nothing rather than something
    /// subtly misplaced.
    pub fn begin_frame(&mut self, target: Extent2D, tolerance: f32) -> &mut Self {
        self.target = target;
        self.tolerance = tolerance;
        self
    }

    pub fn target(&self) -> Extent2D {
        self.target
    }

    /// Tessellate a filled path and map it into clip space.
    ///
    /// `transform` maps path space to **device pixels** — it does not include
    /// the projection onto clip space, which is derived from `target`.
    /// Separating them is what keeps tolerance correct: tolerance is a
    /// device-space quantity, so the scale that matters for flattening is the
    /// path-to-pixel scale alone. Folding the projection in would divide by
    /// the target size and flatten far too coarsely.
    pub fn fill_path(&mut self, path: &Path, transform: Affine2) -> ClipGeometry<'_> {
        let path_tolerance = path_space_tolerance(self.tolerance, &transform);
        let buffers = self.tessellator.fill(path, path_tolerance);
        Self::to_clip_space(&mut self.clip_space, buffers, transform, self.target)
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
    ) -> ClipGeometry<'_> {
        let path_tolerance = path_space_tolerance(self.tolerance, &transform);
        let buffers = self.tessellator.stroke(path, style, path_tolerance);
        Self::to_clip_space(&mut self.clip_space, buffers, transform, self.target)
    }

    /// Tessellate a filled path and append it to a batch.
    ///
    /// The batch is what a backend is handed, so this is the path a real frame
    /// takes: many shapes accumulated, one submission. The borrowing form above
    /// exists for callers inspecting geometry without drawing it.
    pub fn fill_into(
        &mut self,
        batch: &mut Batch,
        path: &Path,
        transform: Affine2,
        paint: &Paint,
    ) -> Result<()> {
        let geo = self.fill_path(path, transform);
        let positions = geo.positions();
        let indices = geo.indices.to_vec();
        batch.push_clipped(
            &positions,
            &indices,
            paint.material.clone(),
            paint.blend,
            paint.clip,
        )
    }

    /// Tessellate a stroked path and append it to a batch.
    pub fn stroke_into(
        &mut self,
        batch: &mut Batch,
        path: &Path,
        style: &StrokeStyle,
        transform: Affine2,
        paint: &Paint,
    ) -> Result<()> {
        let geo = self.stroke_path(path, style, transform);
        let positions = geo.positions();
        let indices = geo.indices.to_vec();
        batch.push_clipped(
            &positions,
            &indices,
            paint.material.clone(),
            paint.blend,
            paint.clip,
        )
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
        r.begin_frame(Extent2D::new(64, 64), TOLERANCE);
        let geo = r.fill_path(&rect(0.0, 0.0, 64.0, 64.0), Affine2::IDENTITY);
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
        r.begin_frame(target, TOLERANCE);
        let plain = r.fill_path(&path, Affine2::IDENTITY).triangle_count();
        let scaled = r
            .fill_path(&path, Affine2::from_scale(Vec2::splat(10.0)))
            .triangle_count();
        assert!(
            scaled > plain,
            "scaling up should refine flattening: {plain} then {scaled}"
        );

        // Target size must not affect it. Folding the projection into the
        // tolerance scale would make a large target flatten coarsely.
        r.begin_frame(Extent2D::new(4096, 4096), TOLERANCE);
        let big_target = r.fill_path(&path, Affine2::IDENTITY).triangle_count();
        assert_eq!(big_target, plain);
    }

    #[test]
    fn an_empty_path_produces_no_geometry() {
        let mut r = Renderer::new();
        r.begin_frame(Extent2D::new(64, 64), TOLERANCE);
        let geo = r.fill_path(&Path::default(), Affine2::IDENTITY);
        assert!(geo.is_empty());
        assert!(geo.positions().is_empty());
    }
}
