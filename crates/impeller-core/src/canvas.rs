//! The recording surface.
//!
//! A canvas is where drawing is expressed. It holds a transform stack and
//! accumulates shapes into a batch, which is what a backend is eventually
//! handed. Recording is separate from submitting so that a whole frame can be
//! described before any of it reaches the GPU — that separation is what lets
//! draws be batched into one pass rather than submitted one at a time.

use crate::paint::{Paint, Shader, Style};
use crate::Color;
use glam::{Affine2, Mat2, Vec2};
use impeller_geometry::transform::{
    preserves_axis_alignment, transformed_bounds, viewport_projection,
};
use impeller_geometry::{Path, PathBuilder};
use impeller_hal::{
    Batch, BlendMode, ClipState, Extent2D, Material, PassDescriptor, Result, Scissor, Stop,
};
use impeller_renderer::{Paint as RenderPaint, Renderer, TOLERANCE};

/// A rectangle in user coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn from_size(width: f32, height: f32) -> Self {
        Self::new(0.0, 0.0, width, height)
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    fn to_path(self) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(self.left, self.top))
            .line_to(Vec2::new(self.right, self.top))
            .line_to(Vec2::new(self.right, self.bottom))
            .line_to(Vec2::new(self.left, self.bottom))
            .close();
        b.build()
    }
}

/// A finished recording, ready to submit.
pub struct Recording {
    pub batch: Batch,
    pub pass: PassDescriptor,
}

impl Recording {
    pub fn draw_count(&self) -> usize {
        self.batch.draw_count()
    }

    pub fn is_empty(&self) -> bool {
        self.batch.is_empty()
    }
}

/// Transform and clip, saved together.
///
/// One stack rather than two: a `save` and its `restore` bracket a subtree, and
/// letting the two pieces of state unwind independently would mean a caller
/// could balance one while leaving the other adrift.
#[derive(Debug, Clone, Copy)]
struct SavedState {
    transform: Affine2,
    clip: Option<Scissor>,
    depth: u32,
}

/// A rectangle covering the whole target, in device pixels.
///
/// Used for the draws that step a stencil clip back, which must reach every
/// pixel the clip could have touched.
fn full_target_path(extent: Extent2D) -> Path {
    Rect::new(0.0, 0.0, extent.width as f32, extent.height as f32).to_path()
}

/// Records drawing commands for one frame.
pub struct Canvas {
    renderer: Renderer,
    batch: Batch,
    transform: Affine2,
    /// The region drawing is confined to, or `None` for the whole target.
    ///
    /// Kept in device pixels rather than user space because that is what it
    /// means: a clip is fixed at the moment it is applied, and a later
    /// transform moves the shapes drawn inside it without moving the clip.
    clip: Option<Scissor>,
    /// How many clips of a shape a scissor cannot express are in force.
    ///
    /// Content draws where the stencil holds this, which is true only where
    /// every one of those clips admitted the pixel.
    depth: u32,
    stack: Vec<SavedState>,
    extent: Extent2D,
    background: Option<Color>,
    /// Set once anything asks for antialiasing.
    ///
    /// Multisampling is a property of the pass, so it cannot vary per shape.
    /// Turning it on for the whole frame when any shape wants it is the
    /// behaviour that surprises least; the alternative silently ignores the
    /// request on some shapes.
    anti_alias: bool,
    samples: u32,
}

impl Canvas {
    /// Start recording for a target of the given size.
    pub fn new(extent: Extent2D) -> Self {
        let mut renderer = Renderer::new();
        renderer.begin_frame(extent, TOLERANCE);
        Self {
            renderer,
            batch: Batch::new(),
            transform: Affine2::IDENTITY,
            clip: None,
            depth: 0,
            stack: Vec::new(),
            extent,
            background: None,
            anti_alias: false,
            samples: 4,
        }
    }

    /// Sample count to use when anything is antialiased.
    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples.max(1);
        self
    }

    pub fn extent(&self) -> Extent2D {
        self.extent
    }

    /// Fill the whole target before drawing anything else.
    pub fn clear(&mut self, color: Color) -> &mut Self {
        self.background = Some(color);
        self
    }

    /// Current transform, mapping user coordinates to device pixels.
    pub fn transform(&self) -> Affine2 {
        self.transform
    }

    /// The region drawing is currently confined to, in device pixels.
    ///
    /// `None` is the whole target.
    pub fn clip(&self) -> Option<Scissor> {
        self.clip
    }

    /// Narrow the clip to a rectangle in user space.
    ///
    /// Intersects with the clip already in force rather than replacing it,
    /// which is what makes the clip stack compose: a subtree can only ever
    /// narrow what its parent allowed. The rectangle goes through the current
    /// transform, so it moves with the coordinate system the caller set up.
    ///
    /// # Rotation
    ///
    /// Returns [`Error::Unsupported`] when the current transform rotates or
    /// skews by anything other than a quarter turn, because the result is then
    /// a rotated quadrilateral and this clip is a rectangle. Refusing is the
    /// point: the tempting alternative is to use the rotated shape's bounding
    /// box, which admits pixels the caller asked to remove, and produces a
    /// picture that is wrong in a way that looks like a rendering bug rather
    /// than like a clip that was never applied. Arbitrary clip shapes need a
    /// stencil pass, which is a separate piece of machinery.
    pub fn clip_rect(&mut self, rect: Rect) -> Result<&mut Self> {
        // Under a transform that keeps rectangles rectangular, this is exactly
        // a scissor, which costs nothing and is exact. Under anything else the
        // result is a rotated quadrilateral, and taking its bounding box would
        // admit pixels the caller asked to remove -- so it goes through the
        // stencil like any other shape. That the fast path survives is the
        // point: a scissor stays the right answer for an axis-aligned clip even
        // now that the general one exists.
        if !preserves_axis_alignment(&self.transform) {
            return self.clip_path(&rect.to_path());
        }
        let (min, max) = transformed_bounds(
            &self.transform,
            Vec2::new(rect.left, rect.top),
            Vec2::new(rect.right, rect.bottom),
        );
        let narrowed = Scissor::from_device_bounds(min.into(), max.into(), self.extent);
        self.clip = Some(match self.clip {
            Some(existing) => existing.intersect(narrowed),
            None => narrowed,
        });
        Ok(self)
    }

    /// Narrow the clip to an arbitrary path in user space.
    ///
    /// Intersects with the clip already in force, like [`Self::clip_rect`], and
    /// like it the path travels through the current transform. The shape is
    /// recorded into the batch as a draw that writes only the stencil, so it
    /// costs one draw where a rectangular clip costs none.
    ///
    /// The path is filled by the same tessellator that fills a drawn shape, so
    /// it obeys the same fill rule and produces the same edges. That matters
    /// for more than consistency: the tessellation is a set of non-overlapping
    /// triangles, which is what lets the stencil step forward by one rather
    /// than needing the parity trick an overlapping fan would.
    pub fn clip_path(&mut self, path: &Path) -> Result<&mut Self> {
        if self.clip.is_some_and(Scissor::is_empty) {
            // Already clipped to nothing; narrowing further changes nothing and
            // the draw would be dropped anyway.
            return Ok(self);
        }
        let paint = RenderPaint {
            // Masked off entirely by a clip draw, but a material is still
            // needed to build one. White says plainly that nothing here is a
            // color decision.
            material: Material::solid([1.0, 1.0, 1.0, 1.0]),
            blend: BlendMode::Src,
            clip: self.clip,
            stencil: ClipState::narrow(self.depth),
        };
        self.renderer
            .fill_into(&mut self.batch, path, self.transform, &paint)?;
        self.depth += 1;
        Ok(self)
    }

    /// Save the transform and clip so a later `restore` can return to them.
    pub fn save(&mut self) -> &mut Self {
        self.stack.push(SavedState {
            transform: self.transform,
            clip: self.clip,
            depth: self.depth,
        });
        self
    }

    /// Return to the most recently saved transform and clip.
    ///
    /// Restoring without a matching save leaves both alone rather than
    /// panicking: an unbalanced pair is a caller bug, but taking down a frame
    /// loop for it is worse than continuing with what is already correct.
    pub fn restore(&mut self) -> &mut Self {
        let Some(previous) = self.stack.pop() else {
            return self;
        };
        self.transform = previous.transform;
        self.clip = previous.clip;

        // A scissor is state the recorder holds, so restoring it is an
        // assignment. A stencil clip lives in a buffer on the device, so
        // restoring it is a draw: one covering the whole target per level being
        // left, each stepping back the pixels that reached that level.
        //
        // Deliberately unscissored. The step back lands exactly where the
        // matching narrowing landed, because that is where the stencil holds
        // the value being tested for -- so it needs no help from a scissor, and
        // asking it to agree with one that has already been restored to
        // something else would be a way to get it wrong.
        if self.depth > previous.depth {
            // Stated in device pixels, so it goes through the identity rather
            // than whatever transform happens to be in force.
            let whole = full_target_path(self.extent);
            while self.depth > previous.depth {
                let paint = RenderPaint {
                    material: Material::solid([1.0, 1.0, 1.0, 1.0]),
                    blend: BlendMode::Src,
                    clip: None,
                    stencil: ClipState::widen(self.depth),
                };
                let _ = self
                    .renderer
                    .fill_into(&mut self.batch, &whole, Affine2::IDENTITY, &paint);
                self.depth -= 1;
            }
        }
        self
    }

    /// How many saves are outstanding, for a caller checking its own balance.
    pub fn save_depth(&self) -> usize {
        self.stack.len()
    }

    pub fn translate(&mut self, x: f32, y: f32) -> &mut Self {
        self.transform *= Affine2::from_translation(Vec2::new(x, y));
        self
    }

    pub fn scale(&mut self, x: f32, y: f32) -> &mut Self {
        self.transform *= Affine2::from_scale(Vec2::new(x, y));
        self
    }

    /// Rotate by an angle in radians.
    pub fn rotate(&mut self, radians: f32) -> &mut Self {
        self.transform *= Affine2::from_angle(radians);
        self
    }

    /// Apply an arbitrary transform on top of the current one.
    pub fn concat(&mut self, transform: Affine2) -> &mut Self {
        self.transform *= transform;
        self
    }

    pub fn draw_path(&mut self, path: &Path, paint: &Paint) -> Result<&mut Self> {
        if !paint.is_visible() || path.is_empty() {
            return Ok(self);
        }
        if paint.anti_alias {
            self.anti_alias = true;
        }

        // A clip narrowed to nothing means this shape cannot reach any pixel.
        // Tessellating it to find that out is wasted work, and it is a normal
        // state rather than an error: a subtree scrolled out of view is clipped
        // away every frame it stays there.
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }

        let render_paint = RenderPaint {
            material: self.material_for(&paint.shader),
            blend: paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        match &paint.style {
            Style::Fill => {
                self.renderer
                    .fill_into(&mut self.batch, path, self.transform, &render_paint)?
            }
            Style::Stroke(stroke) => self.renderer.stroke_into(
                &mut self.batch,
                path,
                stroke,
                self.transform,
                &render_paint,
            )?,
        }
        Ok(self)
    }

    /// Resolve a shader against the current transform.
    ///
    /// Gradient endpoints go through the same mapping the geometry does, so a
    /// gradient rotates and scales with its shape rather than staying fixed to
    /// the screen. Doing it here rather than in the fragment stage means the
    /// shader receives clip-space endpoints and needs no transform of its own.
    fn material_for(&self, shader: &Shader) -> Material {
        let to_clip = viewport_projection(self.extent.width, self.extent.height) * self.transform;
        let stops_of = |stops: &[crate::paint::GradientStop]| -> Vec<Stop> {
            stops
                .iter()
                .map(|s| Stop::new(s.color.to_array(), s.offset))
                .collect()
        };

        match shader {
            Shader::Solid(color) => Material::solid(color.to_array()),
            Shader::Image {
                slot,
                rect,
                alpha,
                tile,
            } => {
                // Texture coordinates run from zero to one across the
                // destination rectangle, so the mapping is: undo the transform
                // that took user space to clip space, then scale by the
                // rectangle's own size. Composing the two here means the shader
                // receives one matrix and does no inversion of its own.
                //
                // A degenerate transform has no inverse, and glam returns a
                // matrix of NaN rather than failing. Those would propagate into
                // texture coordinates and sample nothing in particular, so a
                // collapsed transform maps everything to the image's origin
                // instead -- which is what a zero-area destination looks like
                // anyway.
                let inverse = to_clip.matrix2.inverse();
                let scale = Mat2::from_diagonal(Vec2::new(
                    1.0 / (rect.right - rect.left),
                    1.0 / (rect.bottom - rect.top),
                ));
                let mapping = scale * inverse;
                let origin = to_clip.transform_point2(Vec2::new(rect.left, rect.top));
                let usable = mapping.is_finite() && origin.is_finite();
                Material::Image {
                    origin: if usable { origin.into() } else { [0.0, 0.0] },
                    to_local: if usable {
                        [
                            mapping.x_axis.x,
                            mapping.x_axis.y,
                            mapping.y_axis.x,
                            mapping.y_axis.y,
                        ]
                    } else {
                        [0.0; 4]
                    },
                    slot: *slot,
                    alpha: *alpha,
                    tile: *tile,
                }
            }
            Shader::LinearGradient { start, end, stops } => {
                let start = to_clip.transform_point2(*start);
                let end = to_clip.transform_point2(*end);
                Material::LinearGradient {
                    start: [start.x, start.y],
                    end: [end.x, end.y],
                    stops: stops_of(stops),
                }
            }
            Shader::RadialGradient {
                center,
                radius,
                stops,
            } => {
                let center_clip = to_clip.transform_point2(*center);
                // Folding the radius into the mapping means the shader measures
                // against unit distance and never sees a radius at all.
                let scaled = to_clip.matrix2 * Mat2::from_diagonal(Vec2::splat(*radius));
                Material::RadialGradient {
                    center: [center_clip.x, center_clip.y],
                    to_local: inverse_or_identity(scaled),
                    stops: stops_of(stops),
                }
            }
            Shader::SweepGradient {
                center,
                start_angle,
                end_angle,
                stops,
            } => {
                let center_clip = to_clip.transform_point2(*center);
                Material::SweepGradient {
                    center: [center_clip.x, center_clip.y],
                    to_local: inverse_or_identity(to_clip.matrix2),
                    start_angle: *start_angle,
                    end_angle: *end_angle,
                    stops: stops_of(stops),
                }
            }
        }
    }

    pub fn draw_rect(&mut self, rect: Rect, paint: &Paint) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        let path = rect.to_path();
        self.draw_path(&path, paint)
    }

    /// Draw a circle, approximated by four cubics.
    pub fn draw_circle(&mut self, center: Vec2, radius: f32, paint: &Paint) -> Result<&mut Self> {
        if radius <= 0.0 {
            return Ok(self);
        }
        let path = circle_path(center, radius);
        self.draw_path(&path, paint)
    }

    pub fn draw_line(&mut self, from: Vec2, to: Vec2, paint: &Paint) -> Result<&mut Self> {
        let mut b = PathBuilder::new();
        b.move_to(from).line_to(to);
        let path = b.build();
        self.draw_path(&path, paint)
    }

    /// Finish recording.
    pub fn finish(self) -> Recording {
        let pass = PassDescriptor {
            clear: self.background.map(|c| c.to_array()),
            // One sample unless something asked for antialiasing, because
            // multisampling costs bandwidth and a frame of solid rectangles
            // gains nothing from it.
            samples: if self.anti_alias { self.samples } else { 1 },
        };
        Recording {
            batch: self.batch,
            pass,
        }
    }
}

/// Invert a mapping, falling back to the identity if it cannot be inverted.
///
/// A degenerate transform — a zero scale, or one axis collapsed — has no
/// inverse. That is a caller mistake rather than a renderer one, and the shape
/// it fills is collapsed to nothing anyway, so the gradient it would have
/// carried is not observable. Returning the identity keeps a non-finite matrix
/// out of the shader, where it would spread NaN across every pixel of the draw.
fn inverse_or_identity(matrix: Mat2) -> [f32; 4] {
    let determinant = matrix.determinant();
    let inverse = if determinant.abs() > 1e-9 && determinant.is_finite() {
        matrix.inverse()
    } else {
        Mat2::IDENTITY
    };
    let columns = inverse.to_cols_array();
    if columns.iter().all(|v| v.is_finite()) {
        columns
    } else {
        Mat2::IDENTITY.to_cols_array()
    }
}

/// The constant that makes four cubics approximate a circle.
const KAPPA: f32 = 0.552_284_8;

fn circle_path(center: Vec2, radius: f32) -> Path {
    let (cx, cy) = (center.x, center.y);
    let r = radius;
    let k = KAPPA * r;
    let mut b = PathBuilder::new();
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
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new(Extent2D::new(128, 128))
    }

    #[test]
    fn drawing_accumulates_into_one_batch() {
        let mut canvas = canvas();
        let paint = Paint::fill(Color::WHITE);
        canvas
            .draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &paint)
            .unwrap();
        canvas
            .draw_rect(Rect::new(20.0, 20.0, 30.0, 30.0), &paint)
            .unwrap();

        // Two shapes, one batch: describing the frame before submitting is what
        // lets them share a pass.
        let recording = canvas.finish();
        assert_eq!(recording.draw_count(), 2);
    }

    #[test]
    fn an_invisible_paint_records_nothing() {
        let mut canvas = canvas();
        let invisible = Paint::fill(Color::WHITE.with_alpha(0.0));
        canvas
            .draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &invisible)
            .unwrap();
        canvas
            .draw_circle(Vec2::new(5.0, 5.0), 4.0, &invisible)
            .unwrap();

        // Rejecting early keeps empty geometry out of the batch rather than
        // tessellating it and discovering it was empty.
        assert!(canvas.finish().is_empty());
    }

    #[test]
    fn degenerate_shapes_record_nothing() {
        let mut canvas = canvas();
        let paint = Paint::fill(Color::WHITE);
        canvas
            .draw_rect(Rect::new(10.0, 10.0, 10.0, 20.0), &paint)
            .unwrap();
        canvas.draw_circle(Vec2::ZERO, 0.0, &paint).unwrap();
        assert!(canvas.finish().is_empty());
    }

    #[test]
    fn save_and_restore_return_the_previous_transform() {
        let mut canvas = canvas();
        canvas.translate(10.0, 20.0);
        let outer = canvas.transform();

        canvas.save();
        canvas.scale(3.0, 3.0).rotate(0.5);
        assert_ne!(canvas.transform(), outer);

        canvas.restore();
        assert_eq!(canvas.transform(), outer);
        assert_eq!(canvas.save_depth(), 0);
    }

    #[test]
    fn saves_nest() {
        let mut canvas = canvas();
        let identity = canvas.transform();
        canvas.save();
        canvas.translate(5.0, 0.0);
        canvas.save();
        canvas.translate(5.0, 0.0);
        assert_eq!(canvas.save_depth(), 2);

        canvas.restore();
        assert_eq!(
            canvas.transform(),
            Affine2::from_translation(Vec2::new(5.0, 0.0))
        );
        canvas.restore();
        assert_eq!(canvas.transform(), identity);
    }

    #[test]
    fn restoring_too_often_leaves_the_transform_alone() {
        let mut canvas = canvas();
        canvas.translate(7.0, 3.0);
        let current = canvas.transform();
        // An unbalanced restore is a caller bug, but taking down a frame loop
        // for it is worse than continuing with a transform that is still right.
        canvas.restore().restore().restore();
        assert_eq!(canvas.transform(), current);
    }

    #[test]
    fn transforms_compose_in_the_order_applied() {
        let mut canvas = canvas();
        canvas.translate(10.0, 0.0).scale(2.0, 2.0);
        // Scale then translate, or translate then scale, place a point very
        // differently; the later call applies in the frame the earlier set up.
        let mapped = canvas.transform().transform_point2(Vec2::new(1.0, 0.0));
        assert!((mapped - Vec2::new(12.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn antialiasing_is_off_unless_something_asks_for_it() {
        let mut canvas = canvas();
        let aliased = Paint::fill(Color::WHITE).with_anti_alias(false);
        canvas
            .draw_rect(Rect::from_size(10.0, 10.0), &aliased)
            .unwrap();
        // Multisampling costs bandwidth, and a frame of solid rectangles gains
        // nothing from it.
        assert_eq!(canvas.finish().pass.samples, 1);
    }

    #[test]
    fn one_antialiased_shape_antialiases_the_frame() {
        let mut canvas = canvas();
        canvas
            .draw_rect(
                Rect::from_size(10.0, 10.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .unwrap();
        canvas
            .draw_circle(Vec2::splat(20.0), 8.0, &Paint::fill(Color::WHITE))
            .unwrap();

        // Sampling is a property of the pass, so it cannot vary per shape.
        // Honouring the request for the frame beats silently ignoring it.
        assert!(canvas.finish().pass.samples > 1);
    }

    #[test]
    fn clearing_sets_the_background_rather_than_recording_a_draw() {
        let mut canvas = canvas();
        canvas.clear(Color::rgba8(20, 30, 40, 255));
        let recording = canvas.finish();
        // A clear is a pass property; recording it as a full-target rectangle
        // would cost a draw and defeat the load operation.
        assert!(recording.is_empty());
        assert!(recording.pass.clear.is_some());
    }
}
