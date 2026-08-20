//! The recording surface.
//!
//! A canvas is where drawing is expressed. It holds a transform stack and
//! accumulates shapes into a batch, which is what a backend is eventually
//! handed. Recording is separate from submitting so that a whole frame can be
//! described before any of it reaches the GPU — that separation is what lets
//! draws be batched into one pass rather than submitted one at a time.

use crate::paint::{GradientStop, ImageFilter, MaskBlurStyle, Paint, Shader, Style};
use crate::ramp::Ramp;
use crate::vertices::{Sprite, VertexMode, Vertices};
use crate::Color;
use glam::{Affine2, Mat2, Vec2};
use impeller_geometry::stroke::{LineCap, StrokeStyle};
use impeller_geometry::transform::{
    invert_or_identity, preserves_axis_alignment, transformed_bounds, viewport_projection,
};
use impeller_geometry::{FillRule, Path, PathBuilder};
use impeller_hal::{
    Batch, BlendMode, ClipState, ColorFilter, Error, Extent2D, Material, PassDescriptor, Result,
    Sampling, Scissor, Stop, TileMode, Vertex, MAX_STOPS,
};
use impeller_renderer::{Paint as RenderPaint, Renderer, TOLERANCE};
use impeller_text::{Atlas, PositionedGlyph};

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

    /// This rectangle with its corners rounded to `radius`.
    ///
    /// The radius is clamped to half the shorter side. Larger is not an error
    /// worth refusing -- a caller asking for a fully rounded end usually says
    /// so by passing something enormous -- and left unclamped the corner arcs
    /// would overlap and the outline would cross itself, which fills as
    /// something nobody asked for. Clamped, the shape degenerates to a stadium
    /// and then a circle, which is what asking for a huge radius means.
    ///
    /// A radius at or below zero gives the plain rectangle, so a caller can
    /// pass a radius that happens to be zero without special-casing it.
    pub fn to_rounded_path(self, radius: f32) -> Path {
        if self.is_empty() {
            return Path::default();
        }
        // NaN is checked before clamping rather than after, because `f32::min`
        // ignores a NaN operand: clamping first turns it into half the shorter
        // side, so arithmetic that went wrong would come back as a large
        // rounded rectangle instead of as the plain one asked for.
        //
        // An infinite radius is not rejected with it. It clamps like any other
        // large number, because a caller who writes one means the roundest
        // shape available rather than none at all, and answering with a square
        // would be the opposite of what was asked.
        if radius.is_nan() || radius <= 0.0 {
            return self.to_path();
        }
        let mut path = PathBuilder::new();
        self.add_rounded_contour(&mut path, radius);
        path.build()
    }

    /// Append this rectangle's rounded outline to a builder, as one contour.
    ///
    /// Separate from [`Self::to_rounded_path`] because a shape made of two of
    /// these -- a ring between an outer rectangle and an inner one -- needs
    /// both in one path, and a path built from two paths is not something this
    /// crate offers.
    fn add_rounded_contour(self, path: &mut PathBuilder, radius: f32) {
        let radius = radius.min(self.width() / 2.0).min(self.height() / 2.0);
        // The same constant that makes four cubics a circle, which is what the
        // four corners are: a quarter turn each, at the same radius.
        let k = KAPPA * radius;
        let (l, t, r, b) = (self.left, self.top, self.right, self.bottom);
        path.move_to(Vec2::new(l + radius, t))
            .line_to(Vec2::new(r - radius, t))
            .cubic_to(
                Vec2::new(r - radius + k, t),
                Vec2::new(r, t + radius - k),
                Vec2::new(r, t + radius),
            )
            .line_to(Vec2::new(r, b - radius))
            .cubic_to(
                Vec2::new(r, b - radius + k),
                Vec2::new(r - radius + k, b),
                Vec2::new(r - radius, b),
            )
            .line_to(Vec2::new(l + radius, b))
            .cubic_to(
                Vec2::new(l + radius - k, b),
                Vec2::new(l, b - radius + k),
                Vec2::new(l, b - radius),
            )
            .line_to(Vec2::new(l, t + radius))
            .cubic_to(
                Vec2::new(l, t + radius - k),
                Vec2::new(l + radius - k, t),
                Vec2::new(l + radius, t),
            )
            .close();
    }
}

/// Where a pass's texture slot gets its content.
///
/// A recording is produced without touching a device, so it can name neither a
/// texture nor an allocation. It names either an image the caller will supply
/// or an earlier pass in the same recording, and whoever executes it resolves
/// both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSource {
    /// The caller's image at this index in the table given at draw time.
    Image(u32),
    /// The output of the pass at this index in [`Recording::passes`].
    Layer(usize),
    /// The baked gradient at this index in [`Recording::ramps`].
    ///
    /// Unlike the other two this names something the recorder made rather than
    /// something it was given or rendered, so it is uploaded at submission and
    /// discarded with the frame.
    Ramp(usize),
}

/// One target's worth of drawing.
pub struct Pass {
    pub batch: Batch,
    pub descriptor: PassDescriptor,
    /// What each texture slot this pass samples refers to, in slot order.
    pub sources: Vec<TextureSource>,
    /// The size of the target this pass renders into.
    ///
    /// Per pass rather than per recording, because a layer given bounds
    /// renders into a target the size of those bounds. The root pass always
    /// carries the recording's extent, since that is the caller's surface.
    pub extent: Extent2D,
}

/// How a run of points is joined up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointMode {
    /// Each point on its own, drawn as the stroke's cap.
    #[default]
    Points,
    /// Each pair a segment, and an odd point at the end drawn as nothing.
    Lines,
    /// One open run through all of them.
    Polygon,
}

/// A finished recording, ready to submit.
///
/// More than one pass where the recording used layers. A layer is drawn into a
/// target of its own and then composited back, which cannot happen in the same
/// pass that samples it: reading an attachment being written needs machinery
/// this does not have, and the separation is what makes group opacity mean
/// "make this subtree, then fade it" rather than "fade each shape in it".
pub struct Recording {
    /// Layers first, each before whatever samples it; the root target last.
    ///
    /// The order falls out of how they are recorded rather than being sorted:
    /// a layer is finished when it is restored, which is necessarily before the
    /// draw that composites it.
    pub passes: Vec<Pass>,
    /// The size of the caller's surface, which is what the root pass renders
    /// at. A layer pass may be smaller; see [`Pass::extent`].
    pub extent: Extent2D,
    /// Gradients whose colors were too many to carry in a material, tabulated.
    ///
    /// Recorded here rather than per pass because a slot table is per pass and
    /// this is not: the same gradient drawn into a layer and again over it is
    /// one ramp named twice.
    pub ramps: Vec<Ramp>,
}

impl Recording {
    pub fn draw_count(&self) -> usize {
        self.passes.iter().map(|p| p.batch.draw_count()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.passes.iter().all(|p| p.batch.is_empty())
    }

    /// The pass drawn into the caller's surface.
    pub fn root(&self) -> &Pass {
        self.passes.last().expect("a recording always has a root")
    }

    /// How many offscreen targets executing this needs.
    pub fn layer_count(&self) -> usize {
        self.passes.len() - 1
    }
}

/// The target a pass renders into, placed within the frame.
///
/// A layer with bounds renders into a target the size of those bounds rather
/// than the size of the frame, which is most of the point of giving bounds: a
/// layer covering a tenth of the frame costs a tenth of the memory and a tenth
/// of the fill. Everything that turns a device position into a clip position
/// has to know where that target sits, which is what `origin` carries.
#[derive(Debug, Clone, Copy)]
struct Target {
    /// Where this target's top-left corner sits in the frame, in device pixels.
    /// Whole pixels, so that the composite samples texel centers exactly.
    origin: Vec2,
    extent: Extent2D,
}

impl Target {
    fn frame(extent: Extent2D) -> Self {
        Self {
            origin: Vec2::ZERO,
            extent,
        }
    }

    /// Device position to clip position for this target.
    fn projection(&self) -> Affine2 {
        viewport_projection(self.extent.width, self.extent.height)
            * Affine2::from_translation(-self.origin)
    }

    /// The whole target, in the device space the projection expects.
    fn path(&self) -> Path {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + self.extent.width as f32,
            self.origin.y + self.extent.height as f32,
        )
        .to_path()
    }
}

/// Transform and clip, saved together.
///
/// One stack rather than two: a `save` and its `restore` bracket a subtree, and
/// letting the two pieces of state unwind independently would mean a caller
/// could balance one while leaving the other adrift.
#[derive(Debug)]
struct SavedState {
    transform: Affine2,
    clip: Option<Scissor>,
    clip_bounds: Rect,
    depth: u32,
    /// Set where this save opened a layer, holding what the layer displaced.
    layer: Option<LayerFrame>,
}

/// A layer in progress: the parent's recording, set aside until it returns.
#[derive(Debug)]
struct LayerFrame {
    batch: Batch,
    sources: Vec<TextureSource>,
    paint: Layer,
    /// The target the parent was drawing into, restored when the layer closes.
    parent: Target,
}

/// How a layer is composited back onto what was underneath it.
///
/// A separate type from [`Paint`] rather than a reuse of it, because only two
/// of a paint's parts mean anything here — there is no shape to fill and no
/// geometry to stroke — and a caller handed a `Paint` would reasonably expect
/// its shader to matter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    /// Blur the group before compositing it, with this standard deviation in
    /// device pixels.
    ///
    /// Zero for no blur, which is the default and costs nothing: the extra
    /// passes are only recorded where a caller asked for one.
    ///
    /// On the group rather than on a paint, because a blur is a function of a
    /// finished image and a paint describes one shape. Blurring each shape and
    /// compositing the results is a different picture from blurring the
    /// composite, and the second is what a shadow or a frosted panel means.
    pub blur: f32,
    /// Scales the whole layer on the way back. This is what makes group opacity
    /// differ from per-shape opacity: two overlapping half-transparent shapes
    /// in a layer show one blended edge, where the same shapes drawn directly
    /// show where they cross.
    pub alpha: f32,
    /// How the finished layer meets what was underneath it.
    pub blend: BlendMode,
    /// Transform the finished layer on the way back, in device pixels.
    ///
    /// Distinct from transforming what goes into it, which the canvas's own
    /// transform already does. This resamples the finished image: magnifying
    /// through it gives the layer's own pixels enlarged, where drawing the
    /// same shapes under a larger transform would give them redrawn at the
    /// larger size. That difference is the whole of what a matrix image filter
    /// means -- a caller wanting the sharp one already has the transform
    /// stack.
    ///
    /// `None` composites the layer where it was drawn, which is what every
    /// layer did before this existed.
    pub matrix: Option<Affine2>,
    /// Blur what is already on the target before the layer draws over it.
    ///
    /// This is the other blur, and the difference is which image is filtered.
    /// [`Self::blur`] softens the layer's own content, which is what a shadow
    /// is. This softens what is *behind* it and hands the result to the layer
    /// as its starting content, which is what frosted glass is: a panel that
    /// obscures rather than one that is itself indistinct.
    ///
    /// Zero for none, and it costs nothing when zero. When it is not zero it
    /// costs a copy of the target and two blur passes over it, because reading
    /// what a pass is writing is not something this renderer can do -- see the
    /// architecture note on cutting a pass.
    ///
    /// # Bounds stop being an optimization
    ///
    /// For every other layer, [`Canvas::save_layer_bounds`] only says where the
    /// content is, and the picture is the same either way. For this one the
    /// bounds are the region that gets filtered: a frosted panel is a bounded
    /// layer, and the same layer without bounds blurs the whole frame and
    /// composites the whole frame back. Both are meaningful and they are
    /// different pictures, so state the bounds when you mean a panel.
    pub backdrop_blur: f32,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            blur: 0.0,
            alpha: 1.0,
            blend: BlendMode::SrcOver,
            matrix: None,
            backdrop_blur: 0.0,
        }
    }
}

impl Layer {
    /// Transform the finished layer on the way back. See [`Self::matrix`].
    pub fn with_matrix(mut self, matrix: Affine2) -> Self {
        self.matrix = Some(matrix);
        self
    }

    pub fn opacity(alpha: f32) -> Self {
        Self {
            alpha,
            ..Self::default()
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// Blur the finished group, with `sigma` in device pixels.
    ///
    /// Anything at or below zero, or not a number, means no blur rather than an
    /// error: a caller animating a shadow's softness to nothing should get the
    /// sharp thing, not a refusal at the end of the animation.
    /// Blur the backdrop this layer is drawn over.
    ///
    /// Ignored unless finite and positive, on the same terms as
    /// [`Self::with_blur`]: a sigma that is not a length describes no blur.
    pub fn with_backdrop_blur(mut self, sigma: f32) -> Self {
        self.backdrop_blur = if sigma.is_finite() && sigma > 0.0 {
            sigma
        } else {
            0.0
        };
        self
    }

    pub fn with_blur(mut self, sigma: f32) -> Self {
        self.blur = if sigma.is_finite() && sigma > 0.0 {
            sigma
        } else {
            0.0
        };
        self
    }
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
    /// A rectangle around everything the clip still admits, in device pixels.
    ///
    /// Kept beside the scissor rather than derived from it, because a path
    /// clip narrows what may be drawn without narrowing the scissor at all --
    /// it goes to the stencil instead. A caller asking what is still reachable
    /// wants both accounted for, and only this has seen both.
    clip_bounds: Rect,
    /// How many clips of a shape a scissor cannot express are in force.
    ///
    /// Content draws where the stencil holds this, which is true only where
    /// every one of those clips admitted the pixel.
    depth: u32,
    stack: Vec<SavedState>,
    /// What the batch being recorded samples, in slot order.
    sources: Vec<TextureSource>,
    /// Layers already finished, in the order they were restored.
    ///
    /// A layer is restored before anything can composite it, so this order is
    /// already an order the passes can be executed in.
    finished: Vec<Pass>,
    extent: Extent2D,
    /// The target being recorded into: the frame, or a bounded layer's.
    target: Target,
    background: Option<Color>,
    /// Set once anything asks for antialiasing.
    ///
    /// Multisampling is a property of the pass, so it cannot vary per shape.
    /// Turning it on for the whole frame when any shape wants it is the
    /// behavior that surprises least; the alternative silently ignores the
    /// request on some shapes.
    anti_alias: bool,
    samples: u32,
    /// Gradients tabulated because their stops did not fit in a material.
    ///
    /// Across the whole recording rather than per pass, so the same gradient
    /// drawn into a layer and again over it is baked once.
    ramps: Vec<Ramp>,
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
            // The whole target to start with, which is what an unclipped
            // canvas may reach.
            clip_bounds: Rect::new(0.0, 0.0, extent.width as f32, extent.height as f32),
            depth: 0,
            stack: Vec::new(),
            sources: Vec::new(),
            finished: Vec::new(),
            extent,
            target: Target::frame(extent),
            background: None,
            anti_alias: false,
            samples: 4,
            ramps: Vec::new(),
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

    /// The region drawing may still reach, in device pixels.
    ///
    /// Conservative: it is a rectangle around the clip rather than the clip
    /// itself, so a path clip reports its bounding box and a rotated
    /// rectangle reports the axis-aligned box around it. That is what makes it
    /// useful for the thing it is for -- deciding not to draw something --
    /// since a caller may skip anything outside it and must not assume
    /// everything inside it is visible.
    pub fn destination_clip_bounds(&self) -> Rect {
        self.clip_bounds
    }

    /// The same region in the coordinates the caller is currently drawing in.
    ///
    /// The device bounds carried back through the transform, which under a
    /// rotation gives the box around the rotated box and so grows a little
    /// each time. Conservative in the same direction as everything else here:
    /// too large is a missed optimization, too small is a missing shape.
    ///
    /// A transform that cannot be inverted has collapsed the plane, and
    /// nothing drawn through it reaches anything; the empty rectangle says so.
    pub fn local_clip_bounds(&self) -> Rect {
        let inverse = self.transform.inverse();
        if !inverse.is_finite() {
            return Rect::new(0.0, 0.0, 0.0, 0.0);
        }
        let (min, max) = transformed_bounds(
            &inverse,
            Vec2::new(self.clip_bounds.left, self.clip_bounds.top),
            Vec2::new(self.clip_bounds.right, self.clip_bounds.bottom),
        );
        Rect::new(min.x, min.y, max.x, max.y)
    }

    /// Narrow the tracked bounds to a rectangle already in device space.
    fn narrow_bounds(&mut self, min: Vec2, max: Vec2) {
        let current = self.clip_bounds;
        self.clip_bounds = Rect::new(
            current.left.max(min.x),
            current.top.max(min.y),
            current.right.min(max.x),
            current.bottom.min(max.y),
        );
        // An intersection that crossed over is empty rather than inside out,
        // which every consumer of a rectangle here would otherwise have to
        // check for itself.
        if self.clip_bounds.right < self.clip_bounds.left
            || self.clip_bounds.bottom < self.clip_bounds.top
        {
            self.clip_bounds = Rect::new(
                self.clip_bounds.left,
                self.clip_bounds.top,
                self.clip_bounds.left,
                self.clip_bounds.top,
            );
        }
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
        let narrowed = Scissor::from_device_bounds(
            (min - self.target.origin).into(),
            (max - self.target.origin).into(),
            self.target.extent,
        );
        self.clip = Some(match self.clip {
            Some(existing) => existing.intersect(narrowed),
            None => narrowed,
        });
        self.narrow_bounds(min, max);
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
            filter: ColorFilter::None,
            blend: BlendMode::Src,
            clip: self.clip,
            stencil: ClipState::narrow(self.depth),
        };
        self.renderer
            .fill_into(&mut self.batch, path, self.transform, &paint)?;
        self.depth += 1;
        // A path clip narrows what may be drawn without touching the scissor,
        // so the tracked rectangle is the only place it is accounted for. Its
        // bounding box rather than the path: this rectangle is a promise about
        // what is *outside* it, and a box around a shape keeps that promise.
        let bounds = path.bounds();
        let (min, max) = transformed_bounds(&self.transform, bounds.min, bounds.max);
        self.narrow_bounds(min, max);
        Ok(self)
    }

    /// Save the transform and clip so a later `restore` can return to them.
    pub fn save(&mut self) -> &mut Self {
        self.stack.push(SavedState {
            transform: self.transform,
            clip: self.clip,
            clip_bounds: self.clip_bounds,
            depth: self.depth,
            layer: None,
        });
        self
    }

    /// Begin drawing into a layer of its own, composited back on `restore`.
    ///
    /// Everything recorded until the matching restore goes into a separate
    /// target and arrives as one image, which is what makes an alpha here apply
    /// to the group rather than to each shape in it.
    ///
    /// The layer starts unclipped and at the identity for clipping purposes,
    /// while keeping the current transform. It does not need to inherit the
    /// clip because the draw that composites it is subject to it: content the
    /// clip excludes is discarded once rather than prevented from being drawn,
    /// which costs some work in the layer and keeps a stencil clip from having
    /// to be rebuilt in a second target.
    pub fn save_layer(&mut self, layer: Layer) -> &mut Self {
        let pending = self.open_layer(layer);
        self.seed_backdrop(pending);
        self
    }

    /// Push the layer frame, and cut the backdrop out if one was asked for.
    ///
    /// Returns what the layer's target must be seeded with once its size is
    /// settled, which is why this is separate from the seeding: a bounded layer
    /// does not know its own target until after the frame exists, and the seed
    /// has to land in the target the content will draw into.
    fn open_layer(&mut self, layer: Layer) -> Option<(usize, Target)> {
        let parent = self.target;
        let filtered = (layer.backdrop_blur > 0.0).then(|| {
            let cut = self.cut_pass();
            self.blur_passes(cut, parent, layer.backdrop_blur)
        });
        self.push_layer_frame(layer);
        filtered.map(|index| (index, parent))
    }

    /// End the current target's pass here, and answer which pass now holds it.
    ///
    /// The architecture's rule is that a pass cannot sample the attachment it
    /// is writing, which is exactly what a backdrop filter asks for. So the
    /// pass stops: everything drawn into this target so far becomes a pass of
    /// its own, and what follows begins by drawing that pass back in.
    ///
    /// The redraw is the cost, one full-target copy per backdrop filter, and it
    /// is not avoidable without the machinery the rule exists for the absence
    /// of. It is a `Src` blit covering the whole target, so it reproduces what
    /// was cut exactly rather than compositing with it.
    fn cut_pass(&mut self) -> usize {
        let target = self.target;
        let batch = std::mem::take(&mut self.batch);
        let sources = std::mem::take(&mut self.sources);
        // The clear belongs to the half that starts from nothing. A layer's
        // target clears to transparent and the frame's to its background; after
        // the cut neither clears again, since the redraw below covers every
        // pixel with `Src`.
        let clear = if self.in_layer() {
            Some([0.0; 4])
        } else {
            self.background.take().map(|c| c.to_array())
        };
        self.finished.push(Pass {
            batch,
            descriptor: PassDescriptor {
                clear,
                samples: self.pass_samples(),
            },
            sources,
            extent: target.extent,
        });
        let index = self.finished.len() - 1;
        self.draw_whole_pass(index, target, target, BlendMode::Src);
        index
    }

    /// Whether a layer is open, which decides what a cut pass clears to.
    fn in_layer(&self) -> bool {
        self.stack.iter().any(|state| state.layer.is_some())
    }

    /// Draw a finished pass, sized for `source`, across the whole of `into`.
    ///
    /// `into` is a rectangle of `source`: the same target when a pass is being
    /// redrawn after a cut, and a sub-rectangle of it when a bounded layer is
    /// seeded with a backdrop. The mapping is the inverse of the one a layer
    /// composite uses, and reduces to the full-texture mapping when the two are
    /// the same size, which is what makes the unbounded case share this code.
    fn draw_whole_pass(&mut self, pass: usize, source: Target, into: Target, blend: BlendMode) {
        let slot = self.slot_for(TextureSource::Layer(pass));
        let offset = into.origin - source.origin;
        let (iw, ih) = (into.extent.width as f32, into.extent.height as f32);
        let (sw, sh) = (source.extent.width as f32, source.extent.height as f32);
        let material = Material::Image {
            origin: [-1.0 - 2.0 * offset.x / iw, 1.0 + 2.0 * offset.y / ih],
            to_local: [0.5 * iw / sw, 0.0, 0.0, -0.5 * ih / sh],
            slot,
            alpha: 1.0,
            tile: TileMode::Clamp,
            // A layer is composited at its own size, so a texel lands on a
            // pixel and the filter has nothing to blend. Linear anyway, which
            // is what a target scaled by a resize would want.
            sampling: Sampling::Linear,
            source: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
        };
        let paint = RenderPaint {
            material,
            filter: ColorFilter::None,
            blend,
            // Neither clipped nor stencilled: this is the target's own content
            // being restored or seeded, not something the caller drew, and a
            // clip in force belongs to what comes after it.
            clip: None,
            stencil: ClipState::UNCLIPPED,
        };
        let whole = into.path();
        let _ = self
            .renderer
            .fill_into(&mut self.batch, &whole, Affine2::IDENTITY, &paint);
    }

    /// Put the filtered backdrop under the layer's own content.
    ///
    /// `Src`, because it is the layer's starting image rather than something
    /// composited onto it, and the layer's target was cleared to transparent.
    fn seed_backdrop(&mut self, pending: Option<(usize, Target)>) {
        let Some((pass, parent)) = pending else {
            return;
        };
        let into = self.target;
        self.draw_whole_pass(pass, parent, into, BlendMode::Src);
    }

    fn push_layer_frame(&mut self, layer: Layer) {
        self.stack.push(SavedState {
            transform: self.transform,
            clip: self.clip,
            clip_bounds: self.clip_bounds,
            depth: self.depth,
            layer: Some(LayerFrame {
                batch: std::mem::take(&mut self.batch),
                sources: std::mem::take(&mut self.sources),
                paint: layer,
                parent: self.target,
            }),
        });
        self.clip = None;
        self.depth = 0;
    }

    /// Open a layer that only needs to cover `bounds`.
    ///
    /// `bounds` is in the current user space, and is the caller's promise that
    /// nothing drawn in the layer matters outside it. The layer gets a target
    /// the size of that region rather than the size of the frame, which is the
    /// whole reason to state it: a layer over a tenth of the frame then costs a
    /// tenth of the memory and a tenth of the fill, and a frame that opens
    /// several stops being dominated by full-frame allocations it does not use.
    ///
    /// The promise is enforced by construction rather than trusted. Content
    /// outside the region is clipped away by the target's own edges, so a
    /// caller who understates the bounds sees the drawing cut off — visible and
    /// attributable, rather than reading stale memory.
    ///
    /// The region is taken to device pixels and rounded outward to whole ones,
    /// so a fractional bound never costs coverage at the edge, and is narrowed
    /// to what the enclosing target can show, since a layer larger than that
    /// renders pixels nothing can sample. A region that comes out empty falls
    /// back to a full-size layer, that being a case where a smaller target
    /// would be a guess and guessing wrong loses drawing.
    ///
    /// A layer that filters its backdrop is the exception to all of this: for
    /// it the bounds are not an optimization but the region filtered, so
    /// omitting them filters the whole target rather than merely allocating
    /// more of one. See [`Layer::backdrop_blur`].
    ///
    /// Under a rotation the region becomes a quadrilateral, and the target is
    /// the box around it. That covers more than the caller promised, which is
    /// the safe direction: a target is an allocation rather than a clip the
    /// caller can observe, so over-covering costs a little memory where
    /// under-covering would lose drawing. This is the opposite of what
    /// [`Self::clip_rect`] does with the same box, and for the same reason —
    /// there the box would admit pixels the caller asked to remove.
    pub fn save_layer_bounds(&mut self, layer: Layer, bounds: Rect) -> &mut Self {
        let blur = layer.blur;
        // Opened without seeding, because the seed has to land in the target
        // the content will draw into and that target is decided below. A
        // backdrop drawn into the full-size target and then narrowed would be
        // the wrong region of the wrong image.
        let pending = self.open_layer(layer);
        let (min, max) = transformed_bounds(
            &self.transform,
            Vec2::new(bounds.left, bounds.top),
            Vec2::new(bounds.right, bounds.bottom),
        );
        // A blur reaches past what it was given. The caller states where the
        // content is, which is the question they can answer; how far a blur
        // carries it is this renderer's arithmetic, and a target sized to the
        // content alone would cut the halo off square at the bound -- the
        // failure looking exactly like a shadow with a straight edge.
        //
        // Three deviations, matching where the shader stops taking taps, so the
        // target covers everything the blur will actually read.
        let reach = blur_reach(blur);
        let (min, max) = (min - Vec2::splat(reach), max + Vec2::splat(reach));
        let parent = self.target;
        let left = min.x.floor().max(parent.origin.x);
        let top = min.y.floor().max(parent.origin.y);
        let right = max
            .x
            .ceil()
            .min(parent.origin.x + parent.extent.width as f32);
        let bottom = max
            .y
            .ceil()
            .min(parent.origin.y + parent.extent.height as f32);
        if !(right > left && bottom > top) {
            // The layer keeps the full-size target it was opened with, so the
            // backdrop is seeded across that instead.
            self.seed_backdrop(pending);
            return self;
        }
        self.aim_at(Target {
            origin: Vec2::new(left, top),
            extent: Extent2D::new((right - left) as u32, (bottom - top) as u32),
        });
        self.seed_backdrop(pending);
        self
    }

    /// How many layers are open, for a caller checking its own balance.
    pub fn layer_depth(&self) -> usize {
        self.stack.iter().filter(|s| s.layer.is_some()).count()
    }

    /// The slot a source occupies in the pass being recorded, adding it if new.
    ///
    /// Slots are per pass rather than global, because each pass carries its own
    /// table. Deduplicating means a recording that draws the same image twenty
    /// times binds one texture rather than twenty.
    /// The slot holding this gradient's colors, when they cannot be carried.
    ///
    /// `None` for the ordinary case, which keeps the stops in push constants
    /// and costs no texture, no upload and no binding. Only a gradient with
    /// more stops than a material can hold pays for a ramp, and it pays once
    /// per distinct gradient rather than once per draw.
    fn ramp_for(&mut self, stops: &[GradientStop]) -> Option<u32> {
        (stops.len() > MAX_STOPS).then(|| self.ramp_slot(stops))
    }

    /// Tabulate a gradient's colors and return the slot holding them.
    ///
    /// Called only where the stops outnumber what a material can carry. The
    /// ramp is deduplicated across the recording by its contents, because the
    /// same gradient drawn twice is one texture, and a frame that paints a
    /// hundred list rows with one theme gradient should upload it once.
    fn ramp_slot(&mut self, stops: &[GradientStop]) -> u32 {
        let ramp = Ramp::bake(stops);
        let index = match self.ramps.iter().position(|r| *r == ramp) {
            Some(index) => index,
            None => {
                self.ramps.push(ramp);
                self.ramps.len() - 1
            }
        };
        self.slot_for(TextureSource::Ramp(index))
    }

    fn slot_for(&mut self, source: TextureSource) -> u32 {
        if let Some(index) = self.sources.iter().position(|s| *s == source) {
            return index as u32;
        }
        self.sources.push(source);
        (self.sources.len() - 1) as u32
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
        self.clip_bounds = previous.clip_bounds;

        if let Some(frame) = previous.layer {
            // The clip and stencil are restored before the composite is
            // recorded, so the layer arrives subject to what was in force when
            // it was opened rather than to whatever it did inside itself.
            self.depth = previous.depth;
            self.finish_layer(frame);
            return self;
        }

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
            let whole = self.target.path();
            while self.depth > previous.depth {
                let paint = RenderPaint {
                    material: Material::solid([1.0, 1.0, 1.0, 1.0]),
                    filter: ColorFilter::None,
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
        // Before the mask blur, so the two compose in the order they are
        // defined in: an image filter acts on what was drawn, and what was
        // drawn is whatever the mask blur produced.
        if !paint.image_filter.is_identity() {
            return self.draw_filtered(path, paint);
        }
        if paint.mask_blur > 0.0 {
            return self.draw_masked(path, paint);
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

        // Resolved before the clip is read, since it may add a slot to this
        // pass's table.
        let material = self.material_for(&paint.shader);
        let render_paint = RenderPaint {
            material,
            filter: paint.color_filter,
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
                paint.dash.as_ref(),
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
    fn material_for(&mut self, shader: &Shader) -> Material {
        let to_clip = self.target.projection() * self.transform;
        let stops_of = |stops: &[crate::paint::GradientStop]| -> Vec<Stop> {
            stops
                .iter()
                .map(|s| Stop::new(s.color.to_array(), s.offset))
                .collect()
        };

        match shader {
            Shader::Solid(color) => Material::solid(color.to_array()),
            // Passed through untouched. Every other material here is resolved
            // -- geometry carried into clip space, a radius folded into a
            // matrix -- because this renderer's own shader expects it that
            // way. A caller's program expects whatever the caller wrote, and
            // there is nothing here that could resolve it correctly.
            Shader::RuntimeEffect {
                program,
                uniforms,
                image,
            } => Material::Runtime {
                program: *program,
                uniforms: uniforms.clone(),
                // Through this pass's own table, like every other texture: a
                // layer occupies a slot too, so a caller's index and the
                // pass's are not the same number once one is opened.
                texture: image.map(|slot| self.slot_for(TextureSource::Image(slot))),
            },
            Shader::Image {
                slot,
                rect,
                alpha,
                tile,
                source,
                tint,
                sampling,
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
                    sampling: *sampling,
                    // The caller's index goes through this pass's own table,
                    // because a layer occupies a slot too and the two number
                    // independently. A recording that never uses a layer maps
                    // them one to one, which is why this is invisible until it
                    // is not.
                    slot: self.slot_for(TextureSource::Image(*slot)),
                    alpha: *alpha,
                    source: [source.left, source.top, source.right, source.bottom],
                    tint: tint.to_array(),
                    tile: *tile,
                }
            }
            Shader::LinearGradient {
                start,
                end,
                stops,
                tile,
            } => {
                let axis = *end - *start;
                let start = to_clip.transform_point2(*start);
                if !axis.is_finite() || !start.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::LinearGradient {
                    start: [start.x, start.y],
                    axis: [axis.x, axis.y],
                    // Maps a clip-space offset back into the space the axis is
                    // stated in, which is the caller's. Without it the target's
                    // aspect ratio leaks into the gradient's direction.
                    to_local: invert_or_identity(to_clip.matrix2),
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
            Shader::RadialGradient {
                center,
                radius,
                stops,
                tile,
            } => {
                let center_clip = to_clip.transform_point2(*center);
                // Folding the radius into the mapping means the shader measures
                // against unit distance and never sees a radius at all.
                let scaled = to_clip.matrix2 * Mat2::from_diagonal(Vec2::splat(*radius));
                if !center_clip.is_finite() || !scaled.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::RadialGradient {
                    center: [center_clip.x, center_clip.y],
                    to_local: invert_or_identity(scaled),
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
            Shader::ConicalGradient {
                start_center,
                start_radius,
                end_center,
                end_radius,
                stops,
                tile,
            } => {
                let start_clip = to_clip.transform_point2(*start_center);
                let axis = *end_center - *start_center;
                // The shader is told where the second center is with one float
                // instead of two, which is only possible if it already knows
                // the direction. Rotating the gradient's space so the axis
                // runs along +X is how it comes to know it, and the rotation
                // costs nothing: it folds into a matrix that has to be built
                // and inverted regardless.
                //
                // A zero axis leaves the angle undefined and the rotation
                // arbitrary, which is correct rather than merely harmless --
                // two concentric circles have no direction to preserve, and
                // every rotation is the right one.
                let angle = if axis == Vec2::ZERO {
                    0.0
                } else {
                    axis.y.atan2(axis.x)
                };
                let oriented = to_clip.matrix2 * Mat2::from_angle(angle);
                if !start_clip.is_finite()
                    || !oriented.is_finite()
                    || !start_radius.is_finite()
                    || !end_radius.is_finite()
                {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::ConicalGradient {
                    center: [start_clip.x, start_clip.y],
                    to_local: invert_or_identity(oriented),
                    // In the gradient's own space, which this rotation and the
                    // canvas transform's inverse together make into user space
                    // -- so both radii are the ones the caller stated, and
                    // neither is folded into the matrix the way a radial
                    // gradient's single radius is. A scale can normalize one
                    // radius; it cannot normalize two.
                    start_radius: *start_radius,
                    radius_delta: *end_radius - *start_radius,
                    separation: axis.length(),
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
            Shader::SweepGradient {
                center,
                start_angle,
                end_angle,
                stops,
                tile,
            } => {
                let center_clip = to_clip.transform_point2(*center);
                if !center_clip.is_finite() || !start_angle.is_finite() || !end_angle.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::SweepGradient {
                    center: [center_clip.x, center_clip.y],
                    to_local: invert_or_identity(to_clip.matrix2),
                    start_angle: *start_angle,
                    end_angle: *end_angle,
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
        }
    }

    /// Draw a shape through a blurred layer, which is what a mask blur is.
    ///
    /// Blurring coverage and then filling equals filling and then blurring
    /// exactly when the fill is constant, since a blur is linear. That identity
    /// is the whole implementation: a bounded layer, blurred, with the shape
    /// drawn into it. It also means the machinery is one already tested rather
    /// than a second blur written beside the first.
    ///
    /// The bounds are the shape's, widened by how far the blur reaches. A
    /// caller doing this by hand has to know that reach, which is exactly the
    /// part that is easy to get wrong and shows as a shadow with a straight
    /// edge where it was cut off.
    /// Draw into a layer, filter it, and composite it back.
    ///
    /// The same machinery a mask blur uses, without the restriction: a mask
    /// blur is only the same picture as this when the fill does not vary, and
    /// so takes a solid color, while filtering a result is defined whatever
    /// produced it. What is paid for that is a target of its own.
    fn draw_filtered(&mut self, path: &Path, paint: &Paint) -> Result<&mut Self> {
        let layer = match paint.image_filter {
            ImageFilter::Blur { sigma } => Layer::opacity(1.0).with_blur(sigma),
            ImageFilter::Matrix { transform } => Layer::opacity(1.0).with_matrix(transform),
            // `is_identity` kept `None` out, and every other kind is handled.
            ImageFilter::None => {
                return Err(Error::Unsupported("this image filter is not implemented"))
            }
        };
        let bounds = self.filter_bounds(path, paint);
        self.save_layer_bounds(layer, bounds);
        // Without the filter, or this would open a layer inside itself
        // forever. The mask blur, if there is one, is left on: it applies to
        // the drawing this filter is filtering.
        let inner = paint.clone().with_image_filter(ImageFilter::None);
        let failure = self.draw_path(path, &inner).err();
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// A path's bounds, widened by however far a stroke reaches past it.
    ///
    /// How far a blur reaches is deliberately not added: opening a bounded
    /// layer with a blur already widens the region by that, and the layer is
    /// the right place for it, since it knows its own sigma.
    ///
    /// The stroke's own reach was untested for a while, because that blur
    /// widening covers any stroke narrower than three deviations and every
    /// test had one. It takes a wide stroke and a small blur to tell the two
    /// apart, and there is one now.
    fn filter_bounds(&self, path: &Path, paint: &Paint) -> Rect {
        let bounds = path.bounds();
        let reach = match &paint.style {
            Style::Stroke(stroke) if stroke.width.is_finite() => stroke.width / 2.0,
            _ => 0.0,
        };
        Rect::new(
            bounds.min.x - reach,
            bounds.min.y - reach,
            bounds.max.x + reach,
            bounds.max.y + reach,
        )
    }

    fn draw_masked(&mut self, path: &Path, paint: &Paint) -> Result<&mut Self> {
        if !matches!(paint.shader, Shader::Solid(_)) {
            // See `Paint::mask_blur`: for anything that varies, the two orders
            // are different pictures, and drawing one while the caller asked
            // for the other is the substitution this renderer refuses
            // elsewhere.
            return Err(Error::Unsupported(
                "a mask blur takes a solid color; draw into a blurred layer for anything else",
            ));
        }
        let bounds = self.filter_bounds(path, paint);
        // Without the mask, or this would open a layer inside itself forever.
        let inner = paint.clone().with_mask_blur(0.0);

        // The blurred coverage alone is the whole picture for the default
        // style, so it needs one layer and no second draw.
        if paint.mask_blur_style == MaskBlurStyle::Normal {
            self.save_layer_bounds(Layer::opacity(1.0).with_blur(paint.mask_blur), bounds);
            // The result is discarded to end the borrow before restoring, and
            // taken up again after: the layer has to be closed whether the
            // draw inside it succeeded or not, or every later draw lands in a
            // layer nobody composites.
            let failure = self.draw_path(path, &inner).err();
            self.restore();
            return match failure {
                Some(e) => Err(e),
                None => Ok(self),
            };
        }

        // Every other style combines the blurred coverage with the shape's
        // own, so both have to exist at once, inside a layer that confines the
        // combination. Drawn straight onto the target, a blend that reads the
        // destination would reach what was already there.
        //
        // Which of the two is drawn first is not a matter of taste. A blend
        // only runs where its source produces a fragment, and the shape
        // produces none outside itself -- so a rule that has to *remove*
        // something outside the shape cannot be written with the shape as the
        // source. The blurred layer composites as a quad over the whole
        // region, so it is the operand that can act everywhere, and the two
        // rules needing that are the two where the shape goes down first.

        // The outer layer holds the blur as well as the shape, and it is not
        // itself blurred, so nothing widens it on its behalf.
        let reach = blur_reach(paint.mask_blur);
        let held = Rect::new(
            bounds.left - reach,
            bounds.top - reach,
            bounds.right + reach,
            bounds.bottom + reach,
        );
        let blurred = Layer::opacity(1.0).with_blur(paint.mask_blur);

        self.save_layer_bounds(Layer::opacity(1.0), held);
        let failure = match paint.mask_blur_style {
            // Blur first, then the shape over it. `SrcOver` leaves the blur
            // where the shape is not, and `DstOut` takes the shape out of it
            // -- both of which want the destination untouched outside the
            // shape, which is what a source that draws nothing there gives.
            MaskBlurStyle::Solid | MaskBlurStyle::Outer => {
                let blend = match paint.mask_blur_style {
                    MaskBlurStyle::Solid => BlendMode::SrcOver,
                    _ => BlendMode::DstOut,
                };
                self.save_layer_bounds(blurred, bounds);
                let first = self.draw_path(path, &inner).err();
                self.restore();
                first.or_else(|| self.draw_path(path, &inner.with_blend(blend)).err())
            }
            // The shape first, and the blur composited onto it with `DstIn`.
            // The other order is the obvious one and is wrong: outside the
            // shape there is no fragment for `DstIn` to run on, so the blur
            // survives exactly where this style is supposed to discard it.
            // Compositing a layer covers the whole region, so putting the
            // blur on that side is what makes the rule act everywhere.
            MaskBlurStyle::Inner => {
                let first = self.draw_path(path, &inner).err();
                if first.is_none() {
                    self.save_layer_bounds(blurred.with_blend(BlendMode::DstIn), bounds);
                    let second = self.draw_path(path, &inner).err();
                    self.restore();
                    second
                } else {
                    first
                }
            }
            MaskBlurStyle::Normal => unreachable!("handled above"),
        };
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// Fill or stroke a rectangle.
    ///
    /// An antialiased solid fill takes the same distance field a rounded
    /// rectangle does, with no corner to round. The vertex count is the same
    /// either way -- a rectangle is four vertices whichever route it takes --
    /// so what this buys is the edge: the pass does not have to multisample for
    /// a shape that computes its own coverage, which is four times the fill and
    /// four times the bandwidth saved on a frame made mostly of rectangles.
    pub fn draw_rect(&mut self, rect: Rect, paint: &Paint) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        if let Some(material) = self.analytic_rrect(rect, 0.0, paint) {
            return self.draw_analytic(rect, material, paint);
        }
        let path = rect.to_path();
        self.draw_path(&path, paint)
    }

    /// Fill or stroke a rectangle with rounded corners.
    ///
    /// The shape an interface is mostly made of, and the reason it is here
    /// rather than left to the caller: building it from arcs by hand is a dozen
    /// lines that have to get the corner tangents right, and getting them
    /// slightly wrong shows as a corner that is subtly not round.
    ///
    /// Tessellated like any other path. The analytic coverage that would let a
    /// rounded rectangle skip tessellation entirely is not implemented, and is
    /// where the interesting speed is for a real interface -- this is the shape
    /// that would benefit most from it.
    pub fn draw_rrect(&mut self, rect: Rect, radius: f32, paint: &Paint) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        if let Some(material) = self.analytic_rrect(rect, radius, paint) {
            return self.draw_analytic(rect, material, paint);
        }
        let path = rect.to_rounded_path(radius);
        self.draw_path(&path, paint)
    }

    /// A paint for the fragment-evaluated rounded rectangle, where one applies.
    ///
    /// Only a solid fill that asked for antialiasing. A stroke is a different
    /// shape, a gradient or an image would need both its own mapping and this
    /// one at once — which is the case the push-constant budget was sized
    /// against — and an aliased fill is asking for hard edges, which the
    /// tessellated path gives and this one deliberately does not.
    fn analytic_rrect(&self, rect: Rect, radius: f32, paint: &Paint) -> Option<Material> {
        // Invisible and fully-clipped shapes are rejected before the path is
        // built, and this route has to reject them too. It bypasses
        // `draw_path`, which is where those checks live -- so an alpha of zero
        // went from recording nothing to recording a quad the fragment stage
        // discards a pixel at a time.
        if !paint.is_visible() || self.clip.is_some_and(Scissor::is_empty) {
            return None;
        }
        let stroke = analytic_stroke(paint)?;
        let Shader::Solid(color) = &paint.shader else {
            return None;
        };
        // A radius of zero is a plain rectangle, which this field describes as
        // well as any other: with no corner to round, the expression is the
        // distance to the nearer edge. NaN is not a radius and falls back.
        if radius.is_nan() || radius < 0.0 {
            return None;
        }
        let to_clip = self.target.projection() * self.transform;
        let center = Vec2::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        );
        let center_clip = to_clip.transform_point2(center);
        Some(Material::RoundedRect {
            color: color.to_array(),
            center: [center_clip.x, center_clip.y],
            half_size: [rect.width() / 2.0, rect.height() / 2.0],
            // Maps a clip-space offset from the center back into the shape's
            // own space, so the distance is measured where the radius means
            // what the caller said. Measuring in clip space would round the
            // corners by different amounts on each axis of a target that is
            // not square.
            to_local: invert_or_identity(to_clip.matrix2),
            radius: radius.min(rect.width() / 2.0).min(rect.height() / 2.0),
            stroke,
        })
    }

    /// Draw the quad a fragment-evaluated shape is painted onto.
    ///
    /// Outset by a pixel, because the coverage ramp runs half a pixel either
    /// side of the edge: a quad ending exactly at the shape would clip the
    /// outer half of its own antialiasing and leave a hard edge on a shape
    /// drawn to be soft. Outsetting costs a ring of fragments that compute a
    /// coverage of zero.
    fn draw_analytic(
        &mut self,
        rect: Rect,
        material: Material,
        paint: &Paint,
    ) -> Result<&mut Self> {
        // A pixel for the coverage ramp, plus half the stroke where one is
        // traced: an outline straddles the edge, so it reaches outward by half
        // its width beyond the shape it belongs to.
        let reach = 1.0 + analytic_stroke(paint).unwrap_or(0.0) / 2.0;
        let outset = Rect::new(
            rect.left - reach,
            rect.top - reach,
            rect.right + reach,
            rect.bottom + reach,
        );
        let render_paint = RenderPaint {
            material,
            filter: paint.color_filter,
            blend: paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        self.renderer.fill_into(
            &mut self.batch,
            &outset.to_path(),
            self.transform,
            &render_paint,
        )?;
        Ok(self)
    }

    /// Fill or stroke a circle.
    ///
    /// An antialiased solid fill goes through the same distance field a rounded
    /// rectangle does, because it is one: a square whose corner radius is half
    /// its side has no straight edge left, and the field reduces exactly to the
    /// distance from the center less the radius. So a circle costs two
    /// triangles and needs no shader of its own, where four cubics flattened to
    /// The shadow an object at `elevation` casts, under one light.
    ///
    /// The rule rather than the picture, which is why this is a call and not a
    /// blurred draw a caller assembles: the whole point of an elevation is
    /// that everything at the same height casts a consistent shadow, and
    /// consistency is what a rule spread across call sites loses first.
    ///
    /// The model is a single light above and behind the viewer, which is what
    /// Impeller uses and what makes a raised object's shadow fall downward on
    /// screen. Three things follow from the elevation and nothing else: the
    /// shadow is offset downward by it, blurred in proportion to it, and drawn
    /// at a quarter of the stated colour's alpha.
    ///
    /// `elevation` is in the same units the canvas draws in. Impeller scales
    /// it by a device pixel ratio first, which is a framework concept rather
    /// than a rendering one; a caller who has one should apply it here.
    ///
    /// An opaque occluder hides the part of its own shadow that lies beneath
    /// it, so nothing is drawn there. `transparent_occluder` says the object
    /// will not hide it, and the shadow is drawn whole.
    pub fn draw_shadow(
        &mut self,
        path: &Path,
        color: Color,
        elevation: f32,
        transparent_occluder: bool,
    ) -> Result<&mut Self> {
        if !elevation.is_finite() || elevation <= 0.0 || color.is_invisible() {
            // Nothing at ground level: an object resting on the surface casts
            // no shadow, which is the same answer as an invisible one.
            return Ok(self);
        }

        let sigma = LIGHT_RATIO * elevation;
        let shade = Color::linear(
            color.to_array()[0],
            color.to_array()[1],
            color.to_array()[2],
            color.to_array()[3] * SHADOW_ALPHA,
        );
        let paint = Paint::fill(shade).with_mask_blur(sigma);

        if transparent_occluder {
            // Nothing will cover it, so the whole shadow is part of the
            // picture and no layer is needed to hold anything back.
            self.save();
            self.translate(0.0, elevation);
            let failure = self.draw_path(path, &paint).err();
            self.restore();
            return match failure {
                Some(e) => Err(e),
                None => Ok(self),
            };
        }

        // The part of the shadow the object will cover is spent, so it is
        // taken out. What covers it is the object where it actually sits, not
        // the shadow's own outline -- those are the same shape at different
        // places, and removing the wrong one leaves a crescent of shadow
        // showing above the object and takes a crescent out below it.
        //
        // So this cannot be the outer mask blur style, which removes the shape
        // it blurred. The shadow is blurred whole and the object's own outline
        // is punched out of it afterwards, inside a layer that confines the
        // punch to this shadow rather than to everything already drawn.
        let bounds = self.filter_bounds(path, &paint);
        let reach = blur_reach(sigma);
        let held = Rect::new(
            bounds.left - reach,
            bounds.top - reach,
            bounds.right + reach,
            bounds.bottom + elevation + reach,
        );
        self.save_layer_bounds(Layer::opacity(1.0), held);

        self.save();
        self.translate(0.0, elevation);
        let mut failure = self.draw_path(path, &paint).err();
        self.restore();

        if failure.is_none() {
            let cutter =
                Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_blend(BlendMode::DstOut);
            failure = self.draw_path(path, &cutter).err();
        }
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// a tolerance cost vertices in proportion to how large it is drawn.
    pub fn draw_circle(&mut self, center: Vec2, radius: f32, paint: &Paint) -> Result<&mut Self> {
        // NaN named rather than caught by a negated comparison, which reads as
        // a typo and which clippy objects to on exactly those grounds.
        if radius.is_nan() || radius <= 0.0 {
            return Ok(self);
        }
        let bounds = Rect::new(
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
        );
        if let Some(material) = self.analytic_rrect(bounds, radius, paint) {
            return self.draw_analytic(bounds, material, paint);
        }
        let path = circle_path(center, radius);
        self.draw_path(&path, paint)
    }

    /// Fill or stroke an ellipse inscribed in `bounds`.
    ///
    /// A circle where the bounds are square, and the shape a rounded rectangle
    /// cannot become: past half its shorter side a rounded rectangle stops
    /// changing, giving a stadium, where an ellipse keeps curving on both axes.
    ///
    /// An antialiased solid fill is evaluated per fragment like the other
    /// shapes here. Everything else is tessellated from four cubics, which
    /// approximate an ellipse exactly as well as they approximate a circle --
    /// which is to say closely, and not exactly.
    pub fn draw_oval(&mut self, bounds: Rect, paint: &Paint) -> Result<&mut Self> {
        if bounds.is_empty() {
            return Ok(self);
        }
        if let Some(material) = self.analytic_ellipse(bounds, paint) {
            return self.draw_analytic(bounds, material, paint);
        }
        let path = oval_path(bounds);
        self.draw_path(&path, paint)
    }

    /// A paint for the fragment-evaluated ellipse, where one applies.
    ///
    /// The same conditions the rounded rectangle answers to, and for the same
    /// reasons.
    fn analytic_ellipse(&self, bounds: Rect, paint: &Paint) -> Option<Material> {
        if !paint.is_visible() || self.clip.is_some_and(Scissor::is_empty) {
            return None;
        }
        let stroke = analytic_stroke(paint)?;
        let Shader::Solid(color) = &paint.shader else {
            return None;
        };
        let to_clip = self.target.projection() * self.transform;
        let center = Vec2::new(
            (bounds.left + bounds.right) / 2.0,
            (bounds.top + bounds.bottom) / 2.0,
        );
        let center_clip = to_clip.transform_point2(center);
        Some(Material::Ellipse {
            color: color.to_array(),
            center: [center_clip.x, center_clip.y],
            half_size: [bounds.width() / 2.0, bounds.height() / 2.0],
            to_local: invert_or_identity(to_clip.matrix2),
            stroke,
        })
    }

    /// Draw a run of positioned glyphs from one atlas, as a single draw.
    ///
    /// `atlas_slot` indexes the image table supplied at draw time, and each
    /// glyph names where in that atlas its coverage sits. A run is one draw
    /// however many glyphs it holds, because the coordinates travel on the
    /// vertices rather than in the paint — which is the reason vertices carry
    /// them at all, text being the highest draw-count content there is.
    ///
    /// The paint's color is the text color; its shader is otherwise ignored,
    /// since the atlas supplies coverage rather than color. Gradient-filled
    /// text needs the run drawn into a layer and the gradient drawn through it,
    /// which is a thing a caller can already build out of what is here.
    pub fn draw_glyphs(
        &mut self,
        glyphs: &[PositionedGlyph],
        atlas: &Atlas,
        atlas_slot: u32,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if glyphs.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }

        let color = match &paint.shader {
            Shader::Solid(color) => color.to_array(),
            // A run tints one color, so anything else has no meaning here. The
            // first stop of a gradient is a guess at what was meant, and a
            // guess drawn is worse than a refusal read.
            _ => {
                return Err(Error::Unsupported(
                    "glyphs take a solid color; fill a layer through a gradient instead",
                ))
            }
        };

        let to_clip = self.target.projection() * self.transform;
        let mut vertices = Vec::with_capacity(glyphs.len() * 4);
        let mut indices = Vec::with_capacity(glyphs.len() * 6);
        for glyph in glyphs {
            let Some(rect) = atlas.get(glyph.key) else {
                // A glyph nobody added would sample whatever texel happens to
                // sit at the origin, which draws a plausible smudge. Refusing
                // names the glyph instead.
                return Err(Error::Unsupported(
                    "a glyph in this run is not in the atlas",
                ));
            };
            if rect.width == 0 || rect.height == 0 {
                // A space occupies no texels and needs no quad, but it still
                // travelled through the run rather than being the caller's to
                // filter out.
                continue;
            }

            let [u0, v0, u1, v1] = rect.uv(atlas.size());
            let [x, y] = glyph.position;
            let [w, h] = glyph.size;
            let corners = [
                ([x, y], [u0, v0]),
                ([x + w, y], [u1, v0]),
                ([x + w, y + h], [u1, v1]),
                ([x, y + h], [u0, v1]),
            ];
            let base = vertices.len() as u32;
            for ([px, py], uv) in corners {
                let clip = to_clip.transform_point2(Vec2::new(px, py));
                vertices.push(Vertex::new([clip.x, clip.y], uv));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        if indices.is_empty() {
            return Ok(self);
        }

        let slot = self.slot_for(TextureSource::Image(atlas_slot));
        self.batch.push_mesh(
            &vertices,
            &indices,
            Material::Glyph { color, slot },
            paint.color_filter,
            paint.blend,
            self.clip,
            ClipState::content(self.depth),
        )?;
        Ok(self)
    }

    /// Draw a mesh of triangles the caller supplied.
    ///
    /// The one drawing call whose geometry does not come from tessellating a
    /// shape, which is why it is also the one that can produce triangles this
    /// renderer would never have made: overlapping, degenerate, wound either
    /// way. None of that is checked beyond what [`Vertices`] checks at
    /// construction, because a mesh is a caller saying what to draw rather
    /// than what to draw *around*, and second-guessing it would defeat the
    /// point.
    ///
    /// Not antialiased. Coverage here comes from the rasterizer's own rule
    /// rather than from a distance field or a tessellated feather, and the
    /// interior edges of a mesh are seams between triangles that must not be
    /// feathered at all -- a mesh whose triangles each faded at their borders
    /// would show a lattice of its own construction.
    ///
    /// Texture coordinates, where the mesh carries them, require an image
    /// paint: they say where in a texture each vertex reads, and a paint with
    /// no texture leaves them meaning nothing. The reverse is allowed -- an
    /// image paint with no coordinates maps by position, the same as on any
    /// other shape.
    pub fn draw_vertices(&mut self, mesh: &Vertices, paint: &Paint) -> Result<&mut Self> {
        if mesh.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }

        let textured = !mesh.texture_coords().is_empty();
        let material = match (&paint.shader, textured) {
            (Shader::Image { .. }, true) => self.mesh_material(&paint.shader)?,
            (_, true) => {
                return Err(Error::Unsupported(
                    "a mesh with texture coordinates needs an image paint to read",
                ))
            }
            (_, false) => self.material_for(&paint.shader),
        };

        let to_clip = self.target.projection() * self.transform;
        let coords = mesh.texture_coords();
        let colors = mesh.colors();
        let vertices: Vec<Vertex> = mesh
            .positions()
            .iter()
            .enumerate()
            .map(|(i, position)| {
                let clip = to_clip.transform_point2(*position);
                let uv = coords.get(i).copied().unwrap_or(Vec2::ZERO);
                let vertex = Vertex::new([clip.x, clip.y], [uv.x, uv.y]);
                match colors.get(i) {
                    // Premultiplied here rather than in the shader, because
                    // what the rasterizer interpolates between two vertices is
                    // what gets multiplied in, and interpolating straight color
                    // across an edge whose alpha varies gives a color neither
                    // end asked for.
                    Some(color) => vertex.with_color(premultiplied(*color)),
                    None => vertex,
                }
            })
            .collect();

        self.batch.push_mesh(
            &vertices,
            mesh.indices(),
            material,
            paint.color_filter,
            paint.blend,
            self.clip,
            ClipState::content(self.depth),
        )?;
        Ok(self)
    }

    /// Draw many pieces of one sheet, each with its own transform, in one draw.
    ///
    /// This is `draw_vertices` with the mesh built for you, and building it is
    /// the part worth not getting wrong: each sprite is a quad running from
    /// the origin to its source's size, placed by its transform, with the
    /// source rectangle divided by the sheet's size to become the
    /// coordinates its corners read. A caller assembling that themselves has
    /// four chances per sprite to transpose an axis.
    ///
    /// One draw for the whole batch: a hundred sprites from one sheet differ
    /// only in their vertices, so they have no reason to be a hundred draws.
    /// Anything that would split them -- a different sheet, a different paint
    /// -- is a second call.
    ///
    /// A loop over [`Self::draw_vertices`] would end up as one draw too, since
    /// a batch merges adjacent draws that differ in nothing. What this saves
    /// is the per-sprite arithmetic rather than the draw calls.
    ///
    /// `sheet` is the size of the uploaded texture in texels, which the paint
    /// does not carry: a recording is built without touching a device and has
    /// never seen how large the texture is.
    ///
    /// Per-sprite colors, which `dart:ui` offers here, need a color per vertex
    /// and this vertex format has none. A tint on the paint applies to the
    /// whole batch.
    pub fn draw_atlas(
        &mut self,
        sprites: &[Sprite],
        sheet: Extent2D,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if sprites.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if sheet.width == 0 || sheet.height == 0 {
            return Err(Error::Unsupported(
                "a sprite sheet with no texels has nothing to read",
            ));
        }
        if !sprites.iter().all(|s| s.source.is_finite()) {
            return Err(Error::Unsupported(
                "a sprite's source rectangle is not a finite number",
            ));
        }

        let texels = Vec2::new(sheet.width as f32, sheet.height as f32);
        let mut positions = Vec::with_capacity(sprites.len() * 4);
        let mut coords = Vec::with_capacity(sprites.len() * 4);
        let mut colors = Vec::with_capacity(sprites.len() * 4);
        let mut indices = Vec::with_capacity(sprites.len() * 6);
        for sprite in sprites {
            let source = &sprite.source;
            let size = Vec2::new(source.width, source.height);
            let origin = Vec2::new(source.x, source.y);
            // Corners in the same order for both, so a coordinate and the
            // position it belongs to are written by one step of the loop and
            // cannot come apart.
            let corners = [
                Vec2::ZERO,
                Vec2::new(size.x, 0.0),
                size,
                Vec2::new(0.0, size.y),
            ];
            let base = positions.len() as u32;
            for corner in corners {
                positions.push(sprite.transform.transform_point2(corner));
                coords.push((origin + corner) / texels);
                // The same color at all four corners: a sprite is tinted as a
                // whole, and a gradient across one is a mesh rather than a
                // sprite.
                colors.push(sprite.color);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        let mesh = Vertices::full(VertexMode::Triangles, positions, coords, colors, indices)?;
        self.draw_vertices(&mesh, paint)
    }

    /// The material for a mesh reading a texture at its own coordinates.
    ///
    /// Separate from [`Self::material_for`] because an image drawn on a shape
    /// and an image drawn on a mesh are different materials rather than one
    /// material in two modes -- the mapping, and the source rectangle that
    /// selects part of a sheet, are both things the vertices have already
    /// said. A caller wanting one sprite from a sheet states its texels in the
    /// coordinates.
    fn mesh_material(&mut self, shader: &Shader) -> Result<Material> {
        let Shader::Image {
            slot,
            alpha,
            tile,
            tint,
            source,
            sampling,
            ..
        } = shader
        else {
            return Err(Error::Unsupported("a mesh material needs an image paint"));
        };
        // A source rectangle and a per-vertex coordinate are two answers to
        // the same question, and applying one on top of the other would mean a
        // coordinate of one landed at the rectangle's far edge rather than at
        // the texture's. Refused rather than ignored: a caller who set it
        // meant something by it, and the mesh is where they say it instead.
        if *source != Rect::new(0.0, 0.0, 1.0, 1.0) {
            return Err(Error::Unsupported(
                "a textured mesh states its own coordinates; put the source rectangle in them",
            ));
        }
        let table_slot = self.slot_for(TextureSource::Image(*slot));
        Ok(Material::Mesh {
            slot: table_slot,
            alpha: *alpha,
            tint: tint.to_array(),
            tile: *tile,
            sampling: *sampling,
        })
    }

    /// Draw a run of points, joined as the mode says.
    ///
    /// A point is a segment of no length, which is what decides how it looks:
    /// the stroke's cap is the whole of the shape. A round cap gives a dot the
    /// width across, a square cap a square of that side, and a butt cap --
    /// which adds nothing to either end of a segment -- gives nothing at all.
    /// That is not a special case being handled; it is the only reading of a
    /// zero-length segment that stays consistent with how every other stroke
    /// is drawn.
    ///
    /// Takes the stroke's width even from a paint asking to fill, since there
    /// is no interior to fill and a point with no width is not a shape.
    pub fn draw_points(
        &mut self,
        mode: PointMode,
        points: &[Vec2],
        paint: &Paint,
    ) -> Result<&mut Self> {
        let stroke = match &paint.style {
            Style::Stroke(stroke) => *stroke,
            Style::Fill => StrokeStyle::default(),
        };
        if !stroke.width.is_finite() || stroke.width <= 0.0 {
            return Ok(self);
        }

        match mode {
            PointMode::Points => {
                let radius = stroke.width / 2.0;
                // Filled rather than stroked: what is being drawn is the cap
                // itself, and asking the stroker for a segment of no length is
                // asking it for a direction that does not exist.
                let dot = paint.clone().with_style(Style::Fill);
                for point in points {
                    if !point.is_finite() {
                        continue;
                    }
                    match stroke.cap {
                        LineCap::Round => {
                            self.draw_circle(*point, radius, &dot)?;
                        }
                        LineCap::Square => {
                            self.draw_rect(
                                Rect::new(
                                    point.x - radius,
                                    point.y - radius,
                                    point.x + radius,
                                    point.y + radius,
                                ),
                                &dot,
                            )?;
                        }
                        // A butt cap extends a segment by nothing, and nothing
                        // is what a segment of no length becomes.
                        LineCap::Butt => {}
                    }
                }
            }
            PointMode::Lines => {
                for pair in points.chunks_exact(2) {
                    self.draw_line(pair[0], pair[1], paint)?;
                }
            }
            PointMode::Polygon => {
                if points.len() >= 2 {
                    let mut b = PathBuilder::new();
                    b.move_to(points[0]);
                    for point in &points[1..] {
                        b.line_to(*point);
                    }
                    self.draw_path(&b.build(), paint)?;
                }
            }
        }
        Ok(self)
    }

    /// Fill the ring between two rounded rectangles.
    ///
    /// One path of two contours filled by the even-odd rule, which is what
    /// makes the inner one a hole rather than a second ring drawn on top. A
    /// caller could assemble this, and the reason not to leave them to it is
    /// the rule: two contours wound the same way fill solid under the nonzero
    /// rule and hollow under even-odd, and which one a border needs is not
    /// something to rediscover per call site.
    pub fn draw_drrect(
        &mut self,
        outer: Rect,
        outer_radius: f32,
        inner: Rect,
        inner_radius: f32,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if outer.is_empty() {
            return Ok(self);
        }
        let mut b = PathBuilder::new().with_fill_rule(FillRule::EvenOdd);
        outer.add_rounded_contour(&mut b, outer_radius);
        if !inner.is_empty() {
            inner.add_rounded_contour(&mut b, inner_radius);
        }
        self.draw_path(&b.build(), paint)
    }

    /// Fill everything the clip still admits.
    ///
    /// `dart:ui` calls this `drawPaint`. What it fills is the clip rather than
    /// the target, which is why it is a call and not a rectangle a caller
    /// writes: the rectangle they would need is the clip's bounds carried back
    /// through the current transform, and getting that wrong is invisible
    /// until a transform is in force.
    ///
    /// The bounds are conservative, so this may cover more than the clip
    /// admits -- and the clip then removes the excess, which is what makes a
    /// superset the safe direction to be wrong in.
    pub fn draw_paint(&mut self, paint: &Paint) -> Result<&mut Self> {
        let bounds = self.local_clip_bounds();
        if bounds.is_empty() {
            return Ok(self);
        }
        self.draw_rect(bounds, paint)
    }

    /// Fill everything the clip admits with one color.
    ///
    /// `dart:ui` calls this `drawColor`. Distinct from [`Self::clear`], which
    /// replaces the whole target and ignores the clip: this is a draw, so it
    /// blends and it obeys what is in force.
    pub fn draw_color(&mut self, color: Color, blend: BlendMode) -> Result<&mut Self> {
        self.draw_paint(&Paint::fill(color).with_blend(blend))
    }

    /// Draw an image stretched by its middle, keeping its corners.
    ///
    /// `dart:ui` calls this `drawImageNine`, and it is the shape every
    /// resizable panel with a border is made of. `center` is the part of the
    /// image, in texels, that may stretch; what surrounds it is divided into
    /// eight pieces that stretch along one axis or neither.
    ///
    /// A caller could write the nine draws. The reason not to leave them to it
    /// is the rule rather than the arithmetic: which pieces stretch in which
    /// direction is the whole of what a nine-patch means, and nine call sites
    /// are nine chances to stretch a corner.
    ///
    /// The paint supplies the blend, the filters and whether the edges are
    /// antialiased; its shader is replaced, since each of the nine pieces
    /// needs its own. A caller wanting the whole thing faded should draw it
    /// into a layer, which is what fading a group means everywhere else here.
    ///
    /// A center reaching an edge leaves pieces of no width or height, and
    /// those are skipped rather than drawn empty. A center outside the image
    /// entirely is refused, since there is no reading of it that leaves nine
    /// pieces.
    pub fn draw_image_nine(
        &mut self,
        slot: u32,
        size: Extent2D,
        center: Rect,
        into: Rect,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if size.width == 0 || size.height == 0 || into.is_empty() {
            return Ok(self);
        }
        let (w, h) = (size.width as f32, size.height as f32);
        if center.left < 0.0
            || center.top < 0.0
            || center.right > w
            || center.bottom > h
            || center.right < center.left
            || center.bottom < center.top
        {
            return Err(Error::Unsupported(
                "a nine-patch centre must lie within the image it divides",
            ));
        }

        // The three spans along each axis, in texels and then in the
        // destination. The outer two keep their size and the middle takes
        // whatever is left, which is the whole of the rule.
        let source_x = [0.0, center.left, center.right, w];
        let source_y = [0.0, center.top, center.bottom, h];
        let left_keep = center.left;
        let right_keep = w - center.right;
        let top_keep = center.top;
        let bottom_keep = h - center.bottom;
        // A destination too small to hold both fixed edges would give the
        // middle a negative size, so the edges are scaled down together rather
        // than one of them overrunning the other.
        let squeeze_x = ((into.right - into.left) / (left_keep + right_keep)).min(1.0);
        let squeeze_y = ((into.bottom - into.top) / (top_keep + bottom_keep)).min(1.0);
        let dest_x = [
            into.left,
            into.left + left_keep * squeeze_x,
            into.right - right_keep * squeeze_x,
            into.right,
        ];
        let dest_y = [
            into.top,
            into.top + top_keep * squeeze_y,
            into.bottom - bottom_keep * squeeze_y,
            into.bottom,
        ];

        for row in 0..3 {
            for column in 0..3 {
                let source = Rect::new(
                    source_x[column] / w,
                    source_y[row] / h,
                    source_x[column + 1] / w,
                    source_y[row + 1] / h,
                );
                let destination = Rect::new(
                    dest_x[column],
                    dest_y[row],
                    dest_x[column + 1],
                    dest_y[row + 1],
                );
                if source.is_empty() || destination.is_empty() {
                    continue;
                }
                // The paint supplies everything except the shader: its
                // blend, its filters, whether it antialiases. The shader is
                // this piece's own, because each of the nine reads a different
                // part of the image into a different place, which is the only
                // thing that distinguishes them.
                let piece = paint.clone().with_shader(Shader::Image {
                    slot,
                    rect: destination,
                    alpha: 1.0,
                    tile: TileMode::Clamp,
                    source,
                    tint: Color::WHITE,
                    sampling: Sampling::Linear,
                });
                self.draw_rect(destination, &piece)?;
            }
        }
        Ok(self)
    }

    pub fn draw_line(&mut self, from: Vec2, to: Vec2, paint: &Paint) -> Result<&mut Self> {
        let mut b = PathBuilder::new();
        b.move_to(from).line_to(to);
        let path = b.build();
        self.draw_path(&path, paint)
    }

    /// Close a layer: file its pass, and composite it onto the parent.
    fn finish_layer(&mut self, frame: LayerFrame) {
        let batch = std::mem::replace(&mut self.batch, frame.batch);
        let sources = std::mem::replace(&mut self.sources, frame.sources);

        // A layer clears to transparent rather than to the frame's background:
        // it is composited over what is already there, so anywhere it drew
        // nothing must contribute nothing. Clearing to the background instead
        // would paint an opaque rectangle over the parent.
        let layer = self.target;
        self.aim_at(frame.parent);

        self.finished.push(Pass {
            batch,
            descriptor: PassDescriptor {
                clear: Some([0.0; 4]),
                samples: self.pass_samples(),
            },
            sources,
            extent: layer.extent,
        });
        let mut index = self.finished.len() - 1;
        if frame.paint.blur > 0.0 {
            index = self.blur_passes(index, layer, frame.paint.blur);
        }
        let slot = self.slot_for(TextureSource::Layer(index));

        // Where the layer sits in the parent's clip space, and how much of that
        // space one unit of the layer's texture spans. Clip space runs from -1
        // to 1 and upward, so the axes are halved and Y is negated; the ratio
        // of the two extents is what makes a layer smaller than its parent
        // sample across its own full width rather than a fraction of it.
        //
        // For a full-size layer this comes out to the identity mapping between
        // the target and the image — origin at the top-left of clip space,
        // axes halved — which is what it was before bounds existed.
        let parent = frame.parent;
        let offset = layer.origin - parent.origin;
        let origin = [
            -1.0 + 2.0 * offset.x / parent.extent.width as f32,
            1.0 - 2.0 * offset.y / parent.extent.height as f32,
        ];
        let to_local = [
            0.5 * parent.extent.width as f32 / layer.extent.width as f32,
            0.0,
            0.0,
            -0.5 * parent.extent.height as f32 / layer.extent.height as f32,
        ];
        // A layer asking to be transformed on the way back needs both halves
        // moved, and moving only the geometry is the mistake worth naming: the
        // mapping below carries a fragment's clip position to a texel, so
        // geometry moved without it shows the layer through a window that
        // moved rather than showing a layer that moved.
        // A matrix that folds the plane has no inverse, so there is no mapping
        // saying which texel a fragment reads -- and no geometry either, since
        // the rectangle collapses to a line. Both halves are dropped together:
        // dropping only the mapping leaves a layer drawn through a singular
        // matrix, which is nothing at all, and the fallback would be invisible.
        let placement = frame.paint.matrix.filter(|matrix| {
            let projection = self.target.projection();
            let in_clip = projection * *matrix * projection.inverse();
            in_clip.is_finite() && in_clip.matrix2.inverse().is_finite()
        });
        let (origin, to_local) = match placement {
            None => (origin, to_local),
            Some(matrix) => {
                // The matrix is stated in device pixels; the mapping is in
                // clip space. So it is carried into clip space, the origin
                // goes through it, and the axes take its inverse -- which
                // together say that a fragment reads the texel that landed on
                // it.
                let projection = self.target.projection();
                let in_clip = projection * matrix * projection.inverse();
                let moved = in_clip.transform_point2(Vec2::from(origin));
                let axes = Mat2::from_cols_array(&to_local) * in_clip.matrix2.inverse();
                (moved.into(), axes.to_cols_array())
            }
        };
        let material = Material::Image {
            origin,
            to_local,
            slot,
            alpha: frame.paint.alpha,
            tile: TileMode::Clamp,
            // A layer composites at its own size, so nothing is between texels
            // to choose between.
            sampling: Sampling::Linear,
            source: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
        };
        let paint = RenderPaint {
            material,
            filter: ColorFilter::None,
            blend: frame.paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        // Exactly the layer's own rectangle, stated in device pixels and drawn
        // through the identity: a layer's contents are already where they
        // belong, the transform having been applied to the shapes inside it
        // rather than again to the finished image. Covering more than the layer
        // would sample outside it, which the clamped sampler answers by
        // smearing the edge texels across the rest of the frame.
        let whole = layer.path();
        let _ = self.renderer.fill_into(
            &mut self.batch,
            &whole,
            placement.unwrap_or(Affine2::IDENTITY),
            &paint,
        );
    }

    /// Blur a finished layer, and answer which pass now holds the result.
    ///
    /// Two passes, one per axis, because a two-dimensional Gaussian is the
    /// product of two one-dimensional ones: the same picture as a square of
    /// taps, at a fraction of the work. Each renders a quad covering its whole
    /// target and samples the pass before it.
    ///
    /// Both are the size of the layer, so a bounded layer's blur costs what the
    /// bound says and not what the frame is. The composite that follows is
    /// unchanged, and still applies the layer's alpha and blend -- keeping the
    /// blur passes pure means neither has to know about compositing.
    fn blur_passes(&mut self, source: usize, target: Target, sigma: f32) -> usize {
        // Clip space spans two units and runs upward, so this is the mapping
        // that turns a full-target quad's clip position into the texture
        // coordinates of the pass it samples -- the same pair a layer
        // composite uses at zero offset, since these targets are the same size.
        let origin = [-1.0, 1.0];
        let to_local = [0.5, 0.0, 0.0, -0.5];
        // A step of one texel along each axis, in the sampled texture's own
        // coordinates. The shader cannot derive this: it does not know the size
        // of what it is sampling.
        let steps = [
            [1.0 / target.extent.width as f32, 0.0],
            [0.0, 1.0 / target.extent.height as f32],
        ];

        let mut sampled = source;
        for step in steps {
            let material = Material::Blur {
                origin,
                to_local,
                slot: 0,
                step,
                sigma,
            };
            // A pass of its own, so its slot table starts empty and the one
            // slot it uses is the pass it samples.
            let mut batch = Batch::new();
            let sources = vec![TextureSource::Layer(sampled)];
            let paint = RenderPaint {
                material,
                filter: ColorFilter::None,
                // Replaces rather than blends: the target is cleared and this
                // covers all of it, so anything else would blend against the
                // clear for no reason.
                blend: BlendMode::Src,
                clip: None,
                stencil: ClipState::UNCLIPPED,
            };
            let quad = target.path();
            // The canvas's own renderer, aimed at the blur target for the one
            // draw and put back afterward. A fresh `Renderer` here would build
            // a pair of tessellators per pass per blurred layer per frame, for
            // a quad -- and would be the kind of allocation that never shows up
            // in a profile as itself.
            self.renderer.set_viewport(target.origin, target.extent);
            let _ = self
                .renderer
                .fill_into(&mut batch, &quad, Affine2::IDENTITY, &paint);
            self.renderer
                .set_viewport(self.target.origin, self.target.extent);

            self.finished.push(Pass {
                batch,
                descriptor: PassDescriptor {
                    clear: Some([0.0; 4]),
                    // One sample: this reads a resolved image and writes
                    // another, so multisampling it would resolve twice for no
                    // difference.
                    samples: 1,
                },
                sources,
                extent: target.extent,
            });
            sampled = self.finished.len() - 1;
        }
        sampled
    }

    /// Point both the canvas and its renderer at a target.
    ///
    /// Together rather than separately: the canvas derives a paint's mapping
    /// from the target and the renderer derives the geometry's, and the two
    /// have to agree or a shape lands in one place while its gradient runs
    /// through another.
    fn aim_at(&mut self, target: Target) {
        self.target = target;
        self.renderer.set_viewport(target.origin, target.extent);
    }

    /// The sample count a pass renders at.
    fn pass_samples(&self) -> u32 {
        // One sample unless something asked for antialiasing, because
        // multisampling costs bandwidth and a frame of solid rectangles gains
        // nothing from it.
        if self.anti_alias {
            self.samples
        } else {
            1
        }
    }

    /// Finish recording.
    pub fn finish(mut self) -> Recording {
        // A layer left open is a caller mistake, and the useful recovery is to
        // composite it anyway: the alternative is silently dropping everything
        // drawn since the unbalanced `save_layer`, which looks like a rendering
        // fault rather than like the missing `restore` it is.
        while self.stack.iter().any(|s| s.layer.is_some()) {
            self.restore();
        }

        let samples = self.pass_samples();
        let root = Pass {
            batch: self.batch,
            descriptor: PassDescriptor {
                clear: self.background.map(|c| c.to_array()),
                samples,
            },
            sources: self.sources,
            extent: self.extent,
        };
        let mut passes = self.finished;
        passes.push(root);
        Recording {
            passes,
            extent: self.extent,
            ramps: self.ramps,
        }
    }
}

/// The constant that makes four cubics approximate a circle.
const KAPPA: f32 = 0.552_284_8;

/// The stroke width a paint implies for a fragment-evaluated shape, if one can
/// be drawn that way at all.
///
/// `Some(0.0)` fills, `Some(w)` traces an outline, and `None` means this paint
/// has to be tessellated: without antialiasing there is nothing a distance
/// field offers that triangles do not, and a width that is not finite and
/// positive is not a stroke.
///
/// The joins and caps a stroke style also carries are ignored rather than
/// refused. These shapes are closed curves with no corners to join and no ends
/// to cap, so every setting produces the same outline -- which is why a stroked
/// one can take this path at all.
fn analytic_stroke(paint: &Paint) -> Option<f32> {
    if !paint.anti_alias {
        return None;
    }
    // The shape is drawn on a quad larger than itself and emits a fragment
    // everywhere on it, so a mode that discards the destination where the
    // source is transparent would erase what is behind it in the gap between
    // the two -- and in the corner of a rounded rectangle that gap is most of
    // the corner. Tessellating covers only the shape and has no such gap.
    if !paint.blend.respects_coverage() {
        return None;
    }
    // A distance field describes a continuous outline and has no notion of a
    // position along it, so a dashed stroke cannot be expressed this way at
    // all. Falling back to tessellation is what makes the dash appear; without
    // this the rounded rectangle and the circle would draw a solid outline and
    // silently ignore the pattern, which is the shape of bug the analytic path
    // is most able to hide -- the result looks like a stroke, because it is
    // one.
    if paint.dash.as_ref().is_some_and(|dash| dash.is_usable()) {
        return None;
    }
    // A mask blur is a layer around the draw, and a distance field evaluated on
    // a quad is not a shape that can be put inside one and blurred: the quad is
    // larger than the shape and the layer would blur its edges too.
    if paint.mask_blur > 0.0 {
        return None;
    }
    // An image filter is a layer around the draw for the same reason, and this
    // guard was missing when image filters were added -- so a blurred or moved
    // circle drew as though neither had been asked for. Nothing failed: the
    // shape was still a shape, and only a test comparing where it landed said
    // otherwise. Every route that needs a layer has to be refused here, which
    // is worth stating as the rule rather than as three cases.
    if !paint.image_filter.is_identity() {
        return None;
    }
    match &paint.style {
        Style::Fill => Some(0.0),
        Style::Stroke(stroke) => {
            let width = stroke.width;
            (width.is_finite() && width > 0.0).then_some(width)
        }
    }
}

/// An ellipse inscribed in a rectangle, as four cubics.
///
/// The same construction a circle uses, with a different radius on each axis:
/// the constant that makes a cubic approximate a quarter turn does not care
/// which, since it scales with the axis it belongs to.
fn oval_path(bounds: Rect) -> Path {
    let (cx, cy) = (
        (bounds.left + bounds.right) / 2.0,
        (bounds.top + bounds.bottom) / 2.0,
    );
    let (rx, ry) = (bounds.width() / 2.0, bounds.height() / 2.0);
    let (kx, ky) = (KAPPA * rx, KAPPA * ry);
    let mut b = PathBuilder::new();
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
    b.build()
}

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

/// How far a shadow's blur spreads per unit of elevation.
///
/// The light's radius over its height, which is the ratio that decides how
/// quickly a shadow softens as its caster rises. Impeller writes the same
/// quantity as `800 / 600` -- and in C++ those are integer literals, so the
/// constant there evaluates to one rather than to the one and a third its own
/// comment describes. This uses the ratio the comment states, so a shadow here
/// is a third wider at the same elevation. Recorded rather than matched
/// silently: replicating an apparent typo and correcting one are both
/// decisions, and neither should be made without saying so.
const LIGHT_RATIO: f32 = 800.0 / 600.0;

/// What fraction of the stated colour's alpha a shadow is drawn at.
///
/// A quarter, matching Impeller. A shadow is a suggestion of occlusion rather
/// than an absence of light, and at full alpha it reads as a hole.
const SHADOW_ALPHA: f32 = 0.25;

/// How far past its content a blur of this deviation reaches.
///
/// Three deviations, matching where the shader stops taking taps, so a target
/// sized by this covers everything the blur will actually read.
///
/// Shared rather than written twice. A bounded layer sizes its target with it,
/// and a mask blur style that combines the blurred coverage with the shape's
/// own has to size the layer holding both by the same rule -- which it did not
/// at first, and the halo was cut off square at the shape's own bounds.
fn blur_reach(sigma: f32) -> f32 {
    if sigma > 0.0 {
        (sigma * 3.0).ceil()
    } else {
        0.0
    }
}

/// A color in the form a vertex carries one.
///
/// Premultiplied, because a vertex color is interpolated across a triangle and
/// straight color interpolated between differing alphas is wrong at every point
/// between the ends.
fn premultiplied(color: Color) -> [f32; 4] {
    let [r, g, b, a] = color.to_array();
    [r * a, g * a, b * a, a]
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
        assert_eq!(canvas.finish().root().descriptor.samples, 1);
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
        // A path, because it has to be tessellated. A circle or a rounded
        // rectangle would antialias itself from a distance field and would
        // rightly leave the pass alone -- which the test below is about.
        let mut triangle = PathBuilder::new();
        triangle
            .move_to(Vec2::new(20.0, 20.0))
            .line_to(Vec2::new(40.0, 20.0))
            .line_to(Vec2::new(20.0, 40.0))
            .close();
        canvas
            .draw_path(&triangle.build(), &Paint::fill(Color::WHITE))
            .unwrap();

        // Sampling is a property of the pass, so it cannot vary per shape.
        // Honouring the request for the frame beats silently ignoring it.
        assert!(canvas.finish().root().descriptor.samples > 1);
    }

    #[test]
    fn a_shape_that_antialiases_itself_leaves_the_pass_alone() {
        // The saving that comes with the distance field, and the reason it is
        // worth having beyond the vertex count: a frame whose only antialiased
        // shapes compute their own coverage does not have to multisample, which
        // is four times the fill and four times the bandwidth for an edge it
        // was already going to get right.
        let mut canvas = canvas();
        canvas
            .draw_circle(Vec2::splat(20.0), 8.0, &Paint::fill(Color::WHITE))
            .unwrap();
        canvas
            .draw_rrect(
                Rect::new(30.0, 30.0, 60.0, 50.0),
                6.0,
                &Paint::fill(Color::WHITE),
            )
            .unwrap();
        assert_eq!(
            canvas.finish().root().descriptor.samples,
            1,
            "an analytic shape should not multisample the pass"
        );
    }

    #[test]
    fn clearing_sets_the_background_rather_than_recording_a_draw() {
        let mut canvas = canvas();
        canvas.clear(Color::rgba8(20, 30, 40, 255));
        let recording = canvas.finish();
        // A clear is a pass property; recording it as a full-target rectangle
        // would cost a draw and defeat the load operation.
        assert!(recording.is_empty());
        assert!(recording.root().descriptor.clear.is_some());
    }
}
