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
    invert_or_identity, preserves_axis_alignment, transformed_bounds, viewport_projection,
};
use impeller_geometry::{Path, PathBuilder};
use impeller_hal::{
    Batch, BlendMode, ClipState, Error, Extent2D, Material, PassDescriptor, Result, Scissor, Stop,
    TileMode, Vertex, MAX_STOPS,
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
        let radius = radius.min(self.width() / 2.0).min(self.height() / 2.0);
        // The same constant that makes four cubics a circle, which is what the
        // four corners are: a quarter turn each, at the same radius.
        let k = KAPPA * radius;
        let (l, t, r, b) = (self.left, self.top, self.right, self.bottom);
        let mut path = PathBuilder::new();
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
        path.build()
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
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            blur: 0.0,
            alpha: 1.0,
            blend: BlendMode::SrcOver,
        }
    }
}

impl Layer {
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
}

/// Refuse a gradient carrying more stops than the pipeline can place.
///
/// The material packs a fixed number and used to take the first of them, which
/// draws a gradient that is right up to a point and flat after it -- visibly
/// wrong, plausibly a design decision, and impossible to tell from a correct
/// gradient without counting. That is the shape of mistake this renderer
/// refuses elsewhere: a device without the advanced blend modes reports them
/// unavailable rather than substituting the nearest, and a glyph run given a
/// gradient is refused rather than tinted with its first stop.
///
/// Lifting the limit means baking the stops into a ramp texture and sampling
/// it, which is now possible -- the pipeline samples textures for images,
/// glyphs and blurs -- and is a feature rather than a fix. Until then a caller
/// reads the limit from [`MAX_STOPS`] and resamples its own ramp, which is what
/// this would have to do for it anyway.
///
/// Called from `draw_path` alone, which is every road a gradient can take: the
/// analytic shapes carry a solid color by construction and fall back to
/// tessellation for anything else, so a rounded rectangle filled by a gradient
/// arrives here like any other shape. A second call on that road looked like
/// prudence and was unreachable, which a mutation test showed by deleting it
/// and changing nothing.
fn check_stops(paint: &Paint) -> Result<()> {
    if paint.shader.stop_count() > MAX_STOPS {
        return Err(Error::Unsupported(
            "a gradient carries more color stops than this pipeline can place; \
             see MAX_STOPS",
        ));
    }
    Ok(())
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
            sources: Vec::new(),
            finished: Vec::new(),
            extent,
            target: Target::frame(extent),
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
        let narrowed = Scissor::from_device_bounds(
            (min - self.target.origin).into(),
            (max - self.target.origin).into(),
            self.target.extent,
        );
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
        self.stack.push(SavedState {
            transform: self.transform,
            clip: self.clip,
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
        self
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
    /// Under a rotation the region becomes a quadrilateral, and the target is
    /// the box around it. That covers more than the caller promised, which is
    /// the safe direction: a target is an allocation rather than a clip the
    /// caller can observe, so over-covering costs a little memory where
    /// under-covering would lose drawing. This is the opposite of what
    /// [`Self::clip_rect`] does with the same box, and for the same reason —
    /// there the box would admit pixels the caller asked to remove.
    pub fn save_layer_bounds(&mut self, layer: Layer, bounds: Rect) -> &mut Self {
        let blur = layer.blur;
        self.save_layer(layer);
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
        let reach = if blur > 0.0 { (blur * 3.0).ceil() } else { 0.0 };
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
            return self;
        }
        self.aim_at(Target {
            origin: Vec2::new(left, top),
            extent: Extent2D::new((right - left) as u32, (bottom - top) as u32),
        });
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
        check_stops(paint)?;
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

        // Resolved before the clip is read, since it may add a slot to this
        // pass's table.
        let material = self.material_for(&paint.shader);
        let render_paint = RenderPaint {
            material,
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
                    // The caller's index goes through this pass's own table,
                    // because a layer occupies a slot too and the two number
                    // independently. A recording that never uses a layer maps
                    // them one to one, which is why this is invisible until it
                    // is not.
                    slot: self.slot_for(TextureSource::Image(*slot)),
                    alpha: *alpha,
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
                Material::LinearGradient {
                    start: [start.x, start.y],
                    axis: [axis.x, axis.y],
                    // Maps a clip-space offset back into the space the axis is
                    // stated in, which is the caller's. Without it the target's
                    // aspect ratio leaks into the gradient's direction.
                    to_local: invert_or_identity(to_clip.matrix2),
                    stops: stops_of(stops),
                    tile: *tile,
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
                Material::RadialGradient {
                    center: [center_clip.x, center_clip.y],
                    to_local: invert_or_identity(scaled),
                    stops: stops_of(stops),
                    tile: *tile,
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
                Material::SweepGradient {
                    center: [center_clip.x, center_clip.y],
                    to_local: invert_or_identity(to_clip.matrix2),
                    start_angle: *start_angle,
                    end_angle: *end_angle,
                    stops: stops_of(stops),
                    tile: *tile,
                }
            }
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
            paint.blend,
            self.clip,
            ClipState::content(self.depth),
        )?;
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
        let material = Material::Image {
            origin,
            to_local,
            slot,
            alpha: frame.paint.alpha,
            tile: TileMode::Clamp,
        };
        let paint = RenderPaint {
            material,
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
        let _ = self
            .renderer
            .fill_into(&mut self.batch, &whole, Affine2::IDENTITY, &paint);
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
