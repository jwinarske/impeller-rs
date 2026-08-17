//! The recording surface.
//!
//! A canvas is where drawing is expressed. It holds a transform stack and
//! accumulates shapes into a batch, which is what a backend is eventually
//! handed. Recording is separate from submitting so that a whole frame can be
//! described before any of it reaches the GPU — that separation is what lets
//! draws be batched into one pass rather than submitted one at a time.

use crate::paint::{Paint, Shader, Style};
use crate::Color;
use glam::{Affine2, Vec2};
use impeller_geometry::transform::viewport_projection;
use impeller_geometry::{Path, PathBuilder};
use impeller_hal::{Batch, Extent2D, Material, PassDescriptor, Result, Stop};
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

/// Records drawing commands for one frame.
pub struct Canvas {
    renderer: Renderer,
    batch: Batch,
    transform: Affine2,
    stack: Vec<Affine2>,
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

    /// Save the transform so a later `restore` can return to it.
    pub fn save(&mut self) -> &mut Self {
        self.stack.push(self.transform);
        self
    }

    /// Return to the most recently saved transform.
    ///
    /// Restoring without a matching save leaves the transform alone rather than
    /// panicking: an unbalanced pair is a caller bug, but taking down a frame
    /// loop for it is worse than continuing with what is already correct.
    pub fn restore(&mut self) -> &mut Self {
        if let Some(previous) = self.stack.pop() {
            self.transform = previous;
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

        let render_paint = RenderPaint {
            material: self.material_for(&paint.shader),
            blend: paint.blend,
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
        match shader {
            Shader::Solid(color) => Material::solid(color.to_array()),
            Shader::LinearGradient { start, end, stops } => {
                let to_clip =
                    viewport_projection(self.extent.width, self.extent.height) * self.transform;
                let start = to_clip.transform_point2(*start);
                let end = to_clip.transform_point2(*end);
                Material::LinearGradient {
                    start: [start.x, start.y],
                    end: [end.x, end.y],
                    stops: stops
                        .iter()
                        .map(|s| Stop::new(s.color.to_array(), s.offset))
                        .collect(),
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
