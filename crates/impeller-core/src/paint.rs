//! How a shape is drawn.

use crate::color::Color;
use impeller_geometry::stroke::StrokeStyle;
use impeller_hal::BlendMode;

/// Fill the shape, or trace its outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Style {
    Fill,
    /// Stroke with the given width and joinery.
    ///
    /// The width is in the coordinate space the shape is drawn in, so it scales
    /// with the canvas transform. A caller wanting a hairline that stays one
    /// pixel wide under zoom divides by the current scale.
    Stroke(StrokeStyle),
}

impl Default for Style {
    fn default() -> Self {
        Self::Fill
    }
}

/// Everything about how a shape is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub color: Color,
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
            color: Color::BLACK,
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
            color,
            ..Default::default()
        }
    }

    /// A stroke of the given width, with default caps and joins.
    pub fn stroke(color: Color, width: f32) -> Self {
        Self {
            color,
            style: Style::Stroke(StrokeStyle::new(width)),
            ..Default::default()
        }
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
        if self.color.is_invisible() {
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
    }

    #[test]
    fn a_zero_width_stroke_draws_nothing() {
        // Animating a width to zero should stop drawing rather than emit
        // degenerate geometry for the tessellator to discard.
        assert!(!Paint::stroke(Color::BLACK, 0.0).is_visible());
        assert!(Paint::stroke(Color::BLACK, 0.5).is_visible());
    }
}
