//! How a shape is drawn.

use crate::canvas::Rect;
use crate::color::Color;
use glam::Vec2;
use impeller_geometry::dash::Dash;
use impeller_geometry::stroke::StrokeStyle;
use impeller_hal::{BlendMode, Extent2D, TileMode};
use impeller_hal::{ColorFilter, Sampling};

/// A color stop in a gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub color: Color,
    /// Position along the gradient, from zero to one.
    pub offset: f32,
}

impl GradientStop {
    pub fn new(color: Color, offset: f32) -> Self {
        Self { color, offset }
    }
}

/// What fills a shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Shader {
    Solid(Color),
    /// A gradient along the line between two points **in user space**.
    ///
    /// The endpoints travel through the canvas transform with the geometry, so
    /// a gradient rotates and scales with the shape it fills rather than
    /// staying pinned to the screen.
    LinearGradient {
        start: Vec2,
        end: Vec2,
        stops: Vec<GradientStop>,
        /// What fills the shape beyond the two endpoints.
        ///
        /// A gradient is defined by two points and a shape is rarely exactly
        /// that long, so this is not an edge case: clamping holds the end
        /// colors, repeating tiles the ramp, and decal leaves the outside
        /// empty. Set with [`Paint::with_tile_mode`], the same call an image
        /// uses.
        tile: TileMode,
    },
    /// A gradient outward from a center **in user space**, reaching its last
    /// stop at `radius`.
    RadialGradient {
        center: Vec2,
        radius: f32,
        stops: Vec<GradientStop>,
        /// What fills the shape beyond `radius`. See [`Shader::LinearGradient`].
        tile: TileMode,
    },
    /// A texture, mapped onto a rectangle **in user space**.
    ///
    /// The rectangle travels through the canvas transform with the geometry, so
    /// an image rotates and scales with the shape it fills. It is the region
    /// the image covers, not the region drawn: filling a circle with this paint
    /// draws a circular piece of the image.
    ///
    /// `slot` indexes the table supplied when the recording is drawn. The
    /// canvas names a slot rather than holding a texture because it records
    /// without touching a device, and a backend texture is not something it can
    /// name. Assigning slots is the caller's business for now; a registry that
    /// did it for them is a separate piece of design.
    Image {
        slot: u32,
        rect: Rect,
        /// Scales the sampled color, for drawing an image translucently.
        alpha: f32,
        tile: TileMode,
        /// Which part of the image to draw, from zero to one in each axis.
        ///
        /// The whole image by default. Set it with [`Paint::with_source`], or
        /// with [`Paint::with_source_pixels`] if you would rather state it in
        /// texels and hand over the size you uploaded.
        source: Rect,
        /// Multiplies the sampled color. White changes nothing.
        tint: Color,
        /// How to read between texels. See [`Paint::with_sampling`].
        sampling: Sampling,
    },
    /// A gradient around a center **in user space**, running between two angles
    /// in radians, measured counter-clockwise from the positive X axis.
    SweepGradient {
        center: Vec2,
        start_angle: f32,
        end_angle: f32,
        stops: Vec<GradientStop>,
        /// What fills the directions the arc does not cover.
        ///
        /// A sweep of a full turn covers every direction and this changes
        /// nothing; it is a partial sweep that has an outside.
        tile: TileMode,
    },
    /// A gradient between two circles **in user space**, reaching its first
    /// stop on the first circle and its last on the second.
    ///
    /// This is the general form: a radial gradient is the case where the first
    /// circle is a point at the second's center, and the two-point form is what
    /// `dart:ui` reaches by giving `Gradient.radial` a focal point. It is kept
    /// separate from [`Shader::RadialGradient`] rather than replacing it,
    /// because the radial case folds its radius into a matrix and solves no
    /// quadratic at all, and paying for the general solve on every ordinary
    /// gradient would be a poor trade on the hardware this targets.
    ///
    /// Where no circle in the family passes through a point, nothing is drawn
    /// there -- which is most of the plane when one circle is far outside the
    /// other. That is a property of the two circles rather than of the tile
    /// mode, which acts only where the parameter exists.
    ConicalGradient {
        start_center: Vec2,
        start_radius: f32,
        end_center: Vec2,
        end_radius: f32,
        stops: Vec<GradientStop>,
        /// What fills the parameter outside the two circles. See
        /// [`Shader::LinearGradient`].
        tile: TileMode,
    },
}

impl Shader {
    /// How many color stops this paint carries, or zero where it carries none.
    ///
    /// Public because the threshold is: at or below [`MAX_STOPS`] a gradient
    /// costs nothing beyond its material, and above it the recorder bakes a
    /// small texture. Neither is visible in the result, so this exists for a
    /// caller who cares about the cost rather than the picture.
    ///
    /// [`MAX_STOPS`]: impeller_hal::MAX_STOPS
    pub fn stop_count(&self) -> usize {
        match self {
            Self::Solid(_) | Self::Image { .. } => 0,
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => stops.len(),
        }
    }

    /// Whether this would draw anything at all.
    pub fn is_visible(&self) -> bool {
        match self {
            Self::Solid(color) => !color.is_invisible(),
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => stops.iter().any(|s| !s.color.is_invisible()),
            // What the texture holds is unknown here, so only a zero alpha or
            // an empty destination makes an image provably invisible.
            Self::Image { alpha, rect, .. } => *alpha > 0.0 && !rect.is_empty(),
        }
    }
}

/// A function applied to what a paint drew, rather than to the color it
/// computed.
///
/// The distinction from a color filter is what the input is. A color filter
/// sees one color at a time and cannot know its neighbours; this sees the
/// picture, which is what a blur needs.
///
/// The distinction from [`Paint::mask_blur`] is what is blurred. A mask blur
/// blurs coverage and then fills, which is only the same picture as blurring
/// the result when the fill does not vary -- so it takes a solid color and
/// refuses anything else. This blurs the result, which is defined for every
/// paint and is what `dart:ui` means by an image filter. For a solid color the
/// two agree, and there is a test that says so.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ImageFilter {
    #[default]
    None,
    /// Blur what was drawn, by a standard deviation in device pixels.
    Blur { sigma: f32 },
}

impl ImageFilter {
    /// Whether this would change anything.
    pub fn is_identity(&self) -> bool {
        match self {
            Self::None => true,
            // Stated positively rather than as a negated comparison, which
            // reads badly on a type where two values can be incomparable: a
            // sigma that is not finite is not a blur, and neither is one that
            // is zero or less.
            Self::Blur { sigma } => !sigma.is_finite() || *sigma <= 0.0,
        }
    }
}

/// Fill the shape, or trace its outline.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Style {
    #[default]
    Fill,
    /// Stroke with the given width and joinery.
    ///
    /// The width is in the coordinate space the shape is drawn in, so it scales
    /// with the canvas transform. A caller wanting a hairline that stays one
    /// pixel wide under zoom divides by the current scale.
    Stroke(StrokeStyle),
}

/// Everything about how a shape is painted.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub shader: Shader,
    pub style: Style,
    /// A function applied to what this paint drew, after the shader and any
    /// color filter.
    ///
    /// Costs a layer: what is drawn goes into a target of its own, is
    /// filtered, and is composited back. See [`ImageFilter`] for how this
    /// differs from a color filter and from a mask blur.
    pub image_filter: ImageFilter,
    /// A function applied to the color the shader produces, before blending.
    ///
    /// Beside the shader rather than inside it because it applies to all of
    /// them, and after it rather than before because that is where `dart:ui`
    /// puts a color filter and where it is useful: what is being recolored is
    /// the result of the fill, not the inputs it was built from.
    pub color_filter: ColorFilter,
    /// Cut a stroke into a dash pattern before drawing it.
    ///
    /// On the paint rather than inside [`Style::Stroke`] for two reasons. It
    /// keeps the geometry crate's stroke description a small copyable value,
    /// which the tessellator wants and a pattern of arbitrary length would
    /// end. And it says what it is: a dashed stroke is a stroke of a different
    /// path, so this describes what happens to the path on the way in rather
    /// than how the outline is built.
    ///
    /// Ignored by a fill, which has no length to measure along.
    pub dash: Option<Dash>,
    /// Blur the shape's coverage before filling it, in device pixels.
    ///
    /// What a soft shadow is made of: the same shape, softened, drawn behind
    /// the thing casting it. Zero for none.
    ///
    /// Only a solid color. Blurring coverage and then filling is the same
    /// picture as filling and then blurring exactly when the fill does not
    /// vary, because a blur is linear -- `blur(C·α)` is `C·blur(α)` for a
    /// constant `C` and not for anything else. So a solid paint is drawn
    /// through a blurred layer, which is that identity used, and a gradient or
    /// an image is refused rather than given the other picture and called this
    /// one.
    pub mask_blur: f32,
    pub blend: BlendMode,
    /// Whether to antialias this shape's edges.
    ///
    /// Recorded per paint but applied per pass, because multisampling is a
    /// property of the target rather than of a draw: a canvas that mixes the
    /// two antialiases everything.
    pub anti_alias: bool,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            shader: Shader::Solid(Color::BLACK),
            style: Style::Fill,
            image_filter: ImageFilter::None,
            color_filter: ColorFilter::None,
            dash: None,
            mask_blur: 0.0,
            blend: BlendMode::SrcOver,
            anti_alias: true,
        }
    }
}

impl Paint {
    /// A solid fill.
    pub fn fill(color: Color) -> Self {
        Self {
            shader: Shader::Solid(color),
            ..Default::default()
        }
    }

    /// A fill that runs between colors along a line in user space.
    ///
    /// Any number of them. Up to [`MAX_STOPS`] the colors travel with the
    /// material; beyond that the recorder tabulates them into a texture the
    /// shader samples, which costs one small upload per distinct gradient and
    /// is otherwise invisible. Ask [`Shader::stop_count`] if you want to know
    /// which will happen.
    ///
    /// [`MAX_STOPS`]: impeller_hal::MAX_STOPS
    pub fn linear_gradient(start: Vec2, end: Vec2, stops: Vec<GradientStop>) -> Self {
        Self {
            shader: Shader::LinearGradient {
                start,
                end,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// A fill that runs outward from a center in user space.
    ///
    /// Any number of stops; see [`Paint::linear_gradient`].
    pub fn radial_gradient(center: Vec2, radius: f32, stops: Vec<GradientStop>) -> Self {
        Self {
            shader: Shader::RadialGradient {
                center,
                radius,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// A fill that runs around a center in user space.
    ///
    /// Angles are in radians, counter-clockwise from the positive X axis. At
    /// any number of stops; see [`Paint::linear_gradient`].
    pub fn sweep_gradient(
        center: Vec2,
        start_angle: f32,
        end_angle: f32,
        stops: Vec<GradientStop>,
    ) -> Self {
        Self {
            shader: Shader::SweepGradient {
                center,
                start_angle,
                end_angle,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// A fill that runs between two circles in user space.
    ///
    /// The general gradient: the first stop lands on the first circle and the
    /// last on the second, and every stop between them on a circle
    /// interpolated between the two. `radial_gradient` is the case where the
    /// first circle is a point at the second's center, and is worth reaching
    /// for when that is what you mean -- it does less work per fragment.
    ///
    /// Any number of stops; see [`Paint::linear_gradient`].
    pub fn conical_gradient(
        start_center: Vec2,
        start_radius: f32,
        end_center: Vec2,
        end_radius: f32,
        stops: Vec<GradientStop>,
    ) -> Self {
        Self {
            shader: Shader::ConicalGradient {
                start_center,
                start_radius,
                end_center,
                end_radius,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// How to read a texture between its texels.
    ///
    /// Linear by default, which is what an image drawn at any size but its own
    /// wants. [`Sampling::Nearest`] is for the cases where blending is the
    /// wrong answer: pixel art, and a sprite drawn at exactly its own size
    /// where certainty that no neighbour bled in matters more than smoothness.
    ///
    /// Ignored by a paint with no texture.
    pub fn with_sampling(mut self, sampling: Sampling) -> Self {
        if let Shader::Image { sampling: at, .. } = &mut self.shader {
            *at = sampling;
        }
        self
    }

    /// Filter what this paint draws, rather than the color it computes.
    ///
    /// Unlike a mask blur this takes any shader, because blurring a result is
    /// defined whatever produced it. It is the more expensive of the two: a
    /// layer is allocated, drawn into and composited back, where a mask blur
    /// of a solid color reaches the same picture the same way but is only
    /// offered where that equivalence holds.
    pub fn with_image_filter(mut self, filter: ImageFilter) -> Self {
        self.image_filter = filter;
        self
    }

    /// Apply a function to the color this paint produces, before it blends.
    ///
    /// Works on any shader, which is the point: the image tint that came
    /// before it could only recolor an image. A gradient, a shape, a run of
    /// glyphs all take one now.
    pub fn with_color_filter(mut self, filter: ColorFilter) -> Self {
        self.color_filter = filter;
        self
    }

    /// Blur the shape's coverage, for a shadow or a glow.
    ///
    /// Ignored unless finite and positive. Applies to a fill or a stroke alike
    /// -- what is blurred is whatever coverage the shape produces.
    pub fn with_mask_blur(mut self, sigma: f32) -> Self {
        self.mask_blur = if sigma.is_finite() && sigma > 0.0 {
            sigma
        } else {
            0.0
        };
        self
    }

    /// Dash this paint's stroke, or clear an existing pattern with `None`.
    ///
    /// The lengths are in the space the shape is drawn in, so a pattern scales
    /// with the canvas transform along with the line it cuts. A pattern that
    /// cannot be walked -- empty, negative, or summing to zero -- draws the
    /// line whole rather than drawing nothing, since an undashed line leads
    /// back to the pattern and an absent one leads nowhere.
    pub fn with_dash(mut self, dash: impl Into<Option<Dash>>) -> Self {
        self.dash = dash.into();
        self
    }

    /// A stroke of the given width, with default caps and joins.
    /// A fill that samples an image across a rectangle in user space.
    ///
    /// `slot` indexes the table supplied when the recording is drawn, and
    /// `rect` is the region the image covers rather than the region drawn: the
    /// shape being filled decides what is painted, this decides where the image
    /// sits under it.
    pub fn image(slot: u32, rect: Rect) -> Self {
        Self {
            shader: Shader::Image {
                slot,
                rect,
                alpha: 1.0,
                tile: TileMode::default(),
                source: Rect::new(0.0, 0.0, 1.0, 1.0),
                tint: Color::WHITE,
                sampling: Sampling::default(),
            },
            ..Default::default()
        }
    }

    /// What happens outside the paint's own extent.
    ///
    /// Means the same thing for a gradient as for an image, which is why it is
    /// one call rather than two: past the end of the ramp, past the radius, or
    /// outside the swept arc, the color either holds, or repeats, or stops.
    /// Ignored by a solid paint, which has no outside.
    pub fn with_tile_mode(mut self, tile: TileMode) -> Self {
        match &mut self.shader {
            Shader::Image { tile: current, .. }
            | Shader::LinearGradient { tile: current, .. }
            | Shader::RadialGradient { tile: current, .. }
            | Shader::SweepGradient { tile: current, .. }
            | Shader::ConicalGradient { tile: current, .. } => *current = tile,
            Shader::Solid(_) => {}
        }
        self
    }

    /// Draw only part of the image, in coordinates from zero to one.
    ///
    /// What a sprite sheet needs, and a nine-patch border. Normalized rather
    /// than in texels because a canvas records without touching a device and
    /// has never seen the image: it holds a slot, not a texture, and cannot ask
    /// how large it is. The caller who uploaded it can, which is what
    /// [`Paint::with_source_pixels`] is for.
    ///
    /// Ignored by paints that are not images.
    pub fn with_source(mut self, source: Rect) -> Self {
        if let Shader::Image { source: at, .. } = &mut self.shader {
            *at = source;
        }
        self
    }

    /// The same, stated in texels of an image of the given size.
    ///
    /// A degenerate size would divide by zero, so it leaves the paint alone --
    /// drawing the whole image, which is what the paint already said.
    pub fn with_source_pixels(self, source: Rect, size: Extent2D) -> Self {
        if size.width == 0 || size.height == 0 {
            return self;
        }
        let (w, h) = (size.width as f32, size.height as f32);
        self.with_source(Rect::new(
            source.left / w,
            source.top / h,
            source.right / w,
            source.bottom / h,
        ))
    }

    /// Multiply an image paint's sampled color. Ignored by other paints.
    ///
    /// What turns one monochrome icon sheet into every state a control has:
    /// upload the shapes once as white on transparent, and tint per draw. A
    /// white tint is the identity, which is what an image that never mentions
    /// one already carries.
    ///
    /// Composes with everything else on the paint, so a tinted sprite from a
    /// sheet is one call for the piece and one for the color.
    pub fn with_tint(mut self, tint: Color) -> Self {
        if let Shader::Image { tint: at, .. } = &mut self.shader {
            *at = tint;
        }
        self
    }

    /// Scale an image paint's sampled color. Ignored by other paints.
    pub fn with_image_alpha(mut self, alpha: f32) -> Self {
        if let Shader::Image { alpha: current, .. } = &mut self.shader {
            *current = alpha;
        }
        self
    }

    pub fn stroke(color: Color, width: f32) -> Self {
        Self {
            shader: Shader::Solid(color),
            style: Style::Stroke(StrokeStyle::new(width)),
            ..Default::default()
        }
    }

    pub fn with_shader(mut self, shader: Shader) -> Self {
        self.shader = shader;
        self
    }

    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_anti_alias(mut self, anti_alias: bool) -> Self {
        self.anti_alias = anti_alias;
        self
    }

    /// Whether drawing with this paint would change anything.
    ///
    /// A transparent fill and a zero-width stroke both draw nothing, and
    /// skipping them early keeps empty geometry out of the batch rather than
    /// tessellating it and discovering it was empty.
    pub fn is_visible(&self) -> bool {
        if !self.shader.is_visible() {
            return false;
        }
        match &self.style {
            Style::Fill => true,
            Style::Stroke(stroke) => stroke.is_visible(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_paint_is_an_opaque_antialiased_fill() {
        let paint = Paint::default();
        assert_eq!(paint.style, Style::Fill);
        assert_eq!(paint.blend, BlendMode::SrcOver);
        assert!(paint.anti_alias);
        assert!(paint.is_visible());
    }

    #[test]
    fn a_transparent_paint_draws_nothing_whatever_its_style() {
        let clear = Color::WHITE.with_alpha(0.0);
        assert!(!Paint::fill(clear).is_visible());
        assert!(!Paint::stroke(clear, 4.0).is_visible());

        // A gradient every stop of which is transparent draws nothing either.
        let invisible = Paint::linear_gradient(
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            vec![GradientStop::new(clear, 0.0), GradientStop::new(clear, 1.0)],
        );
        assert!(!invisible.is_visible());
    }

    #[test]
    fn a_zero_width_stroke_draws_nothing() {
        // Animating a width to zero should stop drawing rather than emit
        // degenerate geometry for the tessellator to discard.
        assert!(!Paint::stroke(Color::BLACK, 0.0).is_visible());
        assert!(Paint::stroke(Color::BLACK, 0.5).is_visible());
    }
}
