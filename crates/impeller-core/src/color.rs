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

    /// The four components, linear and straight, in the order everything here
    /// takes them.
    ///
    /// Public because a caller building a color filter needs a color in the
    /// form the filter takes one, and the alternative is a constructor per
    /// filter kind on this type.
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// The sRGB transfer function, inverted, extended below zero by odd symmetry.
///
/// Piecewise: a linear segment near zero and a power curve above it. Using the
/// power curve alone would make near-black values wrong and, worse, its
/// derivative at zero is infinite.
///
/// # Why the split is on the magnitude
///
/// The standard states the curve on `[0, 1]` and states the split between its
/// two segments in terms of the value, which reads as a signed comparison and
/// is not one: which segment applies is decided by how far from zero a value
/// is, not by which side of zero it falls. Compared signed, every negative
/// component took the near-black linear segment -- so `-0.5` decoded to
/// `-0.0387` where the curve's own odd extension gives `-0.2140`.
///
/// Nothing could produce a negative component before a color could leave the
/// sRGB primaries' triangle, so the mistake had nowhere to show. A color stated
/// in a wider gamut is exactly a color that leaves it.
fn srgb_to_linear(value: f32) -> f32 {
    let magnitude = value.abs();
    let decoded = if magnitude <= 0.040_45 {
        magnitude / 12.92
    } else {
        ((magnitude + 0.055) / 1.055).powf(2.4)
    };
    decoded.copysign(value)
}

/// Linear light encoded into sRGB. See [`srgb_to_linear`] for the odd extension.
fn linear_to_srgb(value: f32) -> f32 {
    let magnitude = value.abs();
    let encoded = if magnitude <= 0.003_130_8 {
        magnitude * 12.92
    } else {
        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
    };
    encoded.copysign(value)
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

    /// The curve is odd, so encoding a negative value and negating are the same
    /// operation in either order.
    ///
    /// This is what the standard's piecewise definition extends to below zero,
    /// and what the split on magnitude is for. Compared signed, the near-black
    /// linear segment applied to the whole negative half-line and this fails at
    /// every value past the knee.
    #[test]
    fn the_transfer_function_is_odd_about_zero() {
        for value in [0.001, 0.0031308, 0.01, 0.04045, 0.1, 0.5, 1.0, 1.5] {
            assert!(
                close(linear_to_srgb(-value), -linear_to_srgb(value)),
                "encoding {value}: {} against {}",
                linear_to_srgb(-value),
                -linear_to_srgb(value)
            );
            assert!(
                close(srgb_to_linear(-value), -srgb_to_linear(value)),
                "decoding {value}: {} against {}",
                srgb_to_linear(-value),
                -srgb_to_linear(value)
            );
        }
    }

    /// A color outside the sRGB primaries' triangle has components outside the
    /// unit range, and has to survive the trip out and back like any other.
    #[test]
    fn converting_to_srgb_and_back_returns_the_original_outside_the_unit_range() {
        for value in [
            -1.5, -0.5, -0.226_742, -0.042_057, -0.001, 1.098, 1.225, 1.5,
        ] {
            let round_tripped = linear_to_srgb(srgb_to_linear(value));
            assert!(
                close(round_tripped, value),
                "{value} became {round_tripped}"
            );
        }
    }

    /// The number this matters for, pinned.
    ///
    /// The green component of a Display P3 red, stated in linear sRGB
    /// primaries. Encoding it is where the odd extension shows: the curve gives
    /// -0.2267, and the signed comparison this used to make gave -0.5434 --
    /// more than twice as far from zero, on a value a caller would reasonably
    /// expect to round-trip.
    #[test]
    fn a_component_below_zero_encodes_onto_the_curve_rather_than_its_linear_segment() {
        assert!(close(linear_to_srgb(-0.042_056_955), -0.226_742));
        // Where the old reading put it, kept so the difference is legible.
        assert!(!close(linear_to_srgb(-0.042_056_955), -0.543_376));
    }

    /// Zero stays zero, which the magnitude split has to be checked for
    /// separately: it is the one input where the sign is not recoverable.
    #[test]
    fn zero_is_still_a_fixed_point() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert_eq!(srgb_to_linear(0.0), 0.0);
    }
}
