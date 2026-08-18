//! Color.
//!
//! Linear throughout, with conversion at the API boundary. That split is not
//! cosmetic: blending, filtering, and antialiasing are all averaging
//! operations, and averaging sRGB-encoded values produces results that are
//! visibly too dark — the classic symptom being a gray fringe around
//! antialiased edges on a light background.
//!
//! Callers usually have sRGB values, because that is what design tools and CSS
//! produce, so [`Color::srgb`] converts on the way in and [`Color::to_srgb`]
//! converts back.

/// A linear, straight-alpha color.
///
/// Straight rather than premultiplied at this level because it is what a caller
/// writes; premultiplication happens on the way to the target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::linear(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Self = Self::linear(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Self = Self::linear(1.0, 1.0, 1.0, 1.0);

    /// A color whose components are already linear.
    pub const fn linear(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// A color given in sRGB, converted to linear.
    ///
    /// Alpha is not transformed: it is a coverage fraction rather than a
    /// perceptual quantity, and applying a transfer function to it is a
    /// classic source of washed-out edges.
    pub fn srgb(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r: srgb_to_linear(r),
            g: srgb_to_linear(g),
            b: srgb_to_linear(b),
            a,
        }
    }

    /// A color from the eight-bit sRGB values a design tool produces.
    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::srgb(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        )
    }

    /// Back to sRGB, for reporting or for a caller that needs it.
    pub fn to_srgb(self) -> [f32; 4] {
        [
            linear_to_srgb(self.r),
            linear_to_srgb(self.g),
            linear_to_srgb(self.b),
            self.a,
        ]
    }

    /// Scale alpha, leaving the color itself alone.
    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.a = alpha;
        self
    }

    pub fn is_opaque(self) -> bool {
        self.a >= 1.0
    }

    /// Whether this color would draw nothing.
    pub fn is_invisible(self) -> bool {
        self.a <= 0.0
    }

    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// The sRGB transfer function, inverted.
///
/// Piecewise: a linear segment near zero and a power curve above it. Using the
/// power curve alone would make near-black values wrong and, worse, its
/// derivative at zero is infinite.
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn the_endpoints_are_fixed_points_of_the_transfer_function() {
        for value in [0.0, 1.0] {
            assert!(close(srgb_to_linear(value), value));
            assert!(close(linear_to_srgb(value), value));
        }
    }

    #[test]
    fn converting_to_srgb_and_back_returns_the_original() {
        for value in [0.0, 0.001, 0.05, 0.25, 0.5, 0.75, 0.99, 1.0] {
            let round_tripped = linear_to_srgb(srgb_to_linear(value));
            assert!(
                close(round_tripped, value),
                "{value} became {round_tripped}"
            );
        }
    }

    #[test]
    fn mid_grey_is_not_a_fixed_point() {
        // The whole reason the conversion exists. Half in sRGB is a little over
        // a fifth in linear, and treating them as the same is what makes
        // blended edges too dark.
        let linear = srgb_to_linear(0.5);
        assert!(
            (0.21..0.22).contains(&linear),
            "sRGB 0.5 should be about 0.214 linear, got {linear}"
        );
    }

    #[test]
    fn the_linear_segment_covers_near_black() {
        // A pure power curve has infinite slope at zero, so the standard uses a
        // linear segment there. Without it, very dark values quantise badly.
        assert!(close(srgb_to_linear(0.04), 0.04 / 12.92));
        assert!(srgb_to_linear(0.0001) > 0.0);
    }

    #[test]
    fn alpha_is_not_transformed() {
        // Alpha is coverage, not a perceptual quantity. Running it through the
        // transfer function is a classic cause of washed-out edges.
        let color = Color::srgb(0.5, 0.5, 0.5, 0.5);
        assert_eq!(color.a, 0.5);
        assert_eq!(color.to_srgb()[3], 0.5);
    }

    #[test]
    fn eight_bit_values_land_where_expected() {
        let white = Color::rgba8(255, 255, 255, 255);
        assert!(close(white.r, 1.0) && close(white.a, 1.0));

        let black = Color::rgba8(0, 0, 0, 255);
        assert!(close(black.r, 0.0));

        // A mid gray from a design tool is not half linear.
        let gray = Color::rgba8(128, 128, 128, 255);
        assert!(gray.r < 0.25, "sRGB 128 became {} linear", gray.r);
    }

    #[test]
    fn visibility_is_decided_by_alpha_alone() {
        assert!(Color::TRANSPARENT.is_invisible());
        assert!(!Color::BLACK.is_invisible());
        assert!(Color::BLACK.is_opaque());
        assert!(!Color::WHITE.with_alpha(0.5).is_opaque());
    }
}
