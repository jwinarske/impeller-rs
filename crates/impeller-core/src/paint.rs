//! How a shape is drawn.

use crate::canvas::Rect;
use crate::color::Color;
use glam::Vec2;
use impeller_geometry::stroke::StrokeStyle;
use impeller_hal::{BlendMode, TileMode};

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
    /// A gradient around a center **in user space**, running between two angles
    /// in radians, measured counter-clockwise from the positive X axis.
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
    },
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
}

impl Shader {
    /// Whether this would draw anything at all.
    pub fn is_visible(&self) -> bool {
        match self {
            Self::Solid(color) => !color.is_invisible(),
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. } => stops.iter().any(|s| !s.color.is_invisible()),
            // What the texture holds is unknown here, so only a zero alpha or
            // an empty destination makes an image provably invisible.
            Self::Image { alpha, rect, .. } => *alpha > 0.0 && !rect.is_empty(),
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
    /// Angles are in radians, counter-clockwise from the positive X axis.
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
            | Shader::SweepGradient { tile: current, .. } => *current = tile,
            Shader::Solid(_) => {}
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
