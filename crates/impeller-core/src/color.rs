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

/// The primaries a color's components are stated against.
///
/// A color is three numbers and a rule for reading them, and the rule is this.
/// Two colors with identical components and different spaces are different
/// colors, which is why combining them without saying which space the result is
/// in gives an answer that is subtly wrong rather than obviously so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorSpace {
    /// sRGB primaries, components meant to lie between zero and one.
    #[default]
    Srgb,
    /// sRGB primaries with no range expectation.
    ///
    /// The same primaries as [`Self::Srgb`], so converting between the two
    /// moves nothing: what differs is what a caller means by a component
    /// outside zero to one, not where the color is. This is the space this
    /// renderer works in, and a component outside that range is a color the
    /// sRGB primaries cannot describe rather than a mistake.
    ExtendedSrgb,
    /// Display P3 primaries, which contain the sRGB ones and reach further into
    /// the greens and reds.
    ///
    /// Shares sRGB's white point and its blue primary, so white and pure blue
    /// mean the same thing in both and only the other two axes move.
    DisplayP3,
}

/// A linear, straight-alpha color.
///
/// Straight rather than premultiplied at this level because it is what a caller
/// writes; premultiplication happens on the way to the target.
///
/// The components are linear light in [`Self::space`]'s primaries. Everything
/// below the API works in one space, and [`Self::to_array`] is where a color
/// arrives in it -- so a color stated in another one is converted exactly once,
/// where it is used, rather than at every place that reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    space: ColorSpace,
}

impl Color {
    pub const TRANSPARENT: Self = Self::linear(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Self = Self::linear(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Self = Self::linear(1.0, 1.0, 1.0, 1.0);

    /// A color whose components are already linear, in sRGB primaries.
    pub const fn linear(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r,
            g,
            b,
            a,
            space: ColorSpace::Srgb,
        }
    }

    /// Which primaries this color's components are stated against.
    pub const fn space(self) -> ColorSpace {
        self.space
    }

    /// The same color, its components restated against other primaries.
    ///
    /// Exact in both directions -- a change of primaries is a matrix, and it is
    /// invertible. What it is not is range-preserving: a color inside Display
    /// P3 and outside sRGB comes back with a component below zero or above one,
    /// which is the honest answer rather than a failure. Clamping there would
    /// be the substitution this renderer refuses, and it would be silent.
    pub fn with_space(self, space: ColorSpace) -> Self {
        let [r, g, b] = convert(self.space, space, [self.r, self.g, self.b]);
        Self {
            r,
            g,
            b,
            a: self.a,
            space,
        }
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
            space: ColorSpace::Srgb,
        }
    }

    /// A color given in Display P3, whose components go through the same
    /// transfer function [`Self::srgb`] uses.
    ///
    /// Display P3 shares sRGB's transfer function and its white point and
    /// differs in two of its three primaries, so this decodes exactly as an
    /// sRGB color does and then carries the result against the wider primaries.
    /// Nothing is clipped here or later: a saturated P3 red has no
    /// representation inside the sRGB primaries' triangle, and what
    /// [`Self::to_array`] gives for one has a component below zero. That is the
    /// color, stated in coordinates that cannot contain it.
    pub fn display_p3(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r: srgb_to_linear(r),
            g: srgb_to_linear(g),
            b: srgb_to_linear(b),
            a,
            space: ColorSpace::DisplayP3,
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
        let [r, g, b, a] = self.to_array();
        [linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b), a]
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
    /// The components in the space everything below the API works in.
    ///
    /// This is the one place a color leaves whatever space it was stated in, so
    /// a paint, a gradient stop and a vertex tint all reach the pipeline
    /// against the same primaries however they were written.
    pub fn to_array(self) -> [f32; 4] {
        let [r, g, b] = convert(
            self.space,
            ColorSpace::ExtendedSrgb,
            [self.r, self.g, self.b],
        );
        [r, g, b, self.a]
    }
}

/// Linear Display P3 onto linear sRGB primaries.
///
/// Derived rather than looked up: build each space's matrix onto the CIE
/// tristimulus values from its three published primaries and their shared D65
/// white point, then compose one with the other's inverse. The test does that
/// derivation and compares, so what is trusted here is eight published
/// chromaticities rather than nine constants nobody can check by eye.
///
/// Two properties are visible in the numbers and are worth knowing. Every row
/// sums to one, which is white mapping to white -- the spaces share a white
/// point, so they must agree about it. And the third column is zero except at
/// the bottom, because the two spaces share their blue primary: P3 blue is sRGB
/// blue, at a different luminance.
const P3_TO_SRGB: [[f32; 3]; 3] = [
    [1.224_940_2, -0.224_940_18, 0.0],
    [-0.042_056_95, 1.042_056_9, 0.0],
    [-0.019_637_55, -0.078_636_05, 1.098_273_6],
];

/// The inverse of [`P3_TO_SRGB`], derived the same way.
const SRGB_TO_P3: [[f32; 3]; 3] = [
    [0.822_461_96, 0.177_538_04, 0.0],
    [0.033_194_2, 0.966_805_8, 0.0],
    [0.017_082_63, 0.072_397_44, 0.910_519_93],
];

fn apply(matrix: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    let row = |i: usize| matrix[i][0] * c[0] + matrix[i][1] * c[1] + matrix[i][2] * c[2];
    [row(0), row(1), row(2)]
}

/// Restate linear components from one set of primaries against another.
///
/// `Srgb` and `ExtendedSrgb` name the same primaries, so anything between them
/// is the identity and says so rather than multiplying by a matrix that is one.
fn convert(from: ColorSpace, to: ColorSpace, c: [f32; 3]) -> [f32; 3] {
    match (from, to) {
        (ColorSpace::DisplayP3, ColorSpace::DisplayP3) => c,
        (ColorSpace::DisplayP3, _) => apply(&P3_TO_SRGB, c),
        (_, ColorSpace::DisplayP3) => apply(&SRGB_TO_P3, c),
        _ => c,
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

    /// The matrix in the source, rebuilt from the numbers a reader can look up.
    ///
    /// Nine constants are not checkable by eye, and copying them from somewhere
    /// only moves the question. The eight chromaticities and the white point
    /// below are published and stable, and the derivation from them is short:
    /// build each space's matrix onto the tristimulus values, then compose one
    /// with the other's inverse. What this test trusts is the inputs.
    mod derivation {
        /// Columns are the primaries as tristimulus values, scaled so that
        /// equal components give the white point.
        fn to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
            let column = |p: [f64; 2]| [p[0] / p[1], 1.0, (1.0 - p[0] - p[1]) / p[1]];
            let m = primaries.map(column);
            let w = column(white);
            // Solve `m^T * s = w` for the per-primary scales by Cramer's rule,
            // which at three by three is shorter than any decomposition.
            let det = |a: [[f64; 3]; 3]| {
                a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
                    - a[1][0] * (a[0][1] * a[2][2] - a[0][2] * a[2][1])
                    + a[2][0] * (a[0][1] * a[1][2] - a[0][2] * a[1][1])
            };
            let base = det(m);
            let scale = |i: usize| {
                let mut swapped = m;
                swapped[i] = w;
                det(swapped) / base
            };
            let columns = [
                m[0].map(|v| v * scale(0)),
                m[1].map(|v| v * scale(1)),
                m[2].map(|v| v * scale(2)),
            ];
            // Transposed on the way out: the primaries are natural to build as
            // columns and everything downstream wants rows.
            let mut rows = [[0.0; 3]; 3];
            for (r, row) in rows.iter_mut().enumerate() {
                for (c, cell) in row.iter_mut().enumerate() {
                    *cell = columns[c][r];
                }
            }
            rows
        }

        fn inverse(a: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
            let c = |i: usize, j: usize| {
                let (i0, i1) = ((i + 1) % 3, (i + 2) % 3);
                let (j0, j1) = ((j + 1) % 3, (j + 2) % 3);
                a[i0][j0] * a[i1][j1] - a[i0][j1] * a[i1][j0]
            };
            let det = a[0][0] * c(0, 0) + a[1][0] * c(1, 0) + a[2][0] * c(2, 0);
            // The adjugate is the *transpose* of the cofactor matrix, so the
            // indices swap here. Getting that backwards gives a matrix that is
            // still plausible -- it inverts a symmetric one correctly -- and is
            // wrong for every other.
            let mut out = [[0.0; 3]; 3];
            for (i, row) in out.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell = c(j, i) / det;
                }
            }
            out
        }

        /// Rows of the result, as the source states them.
        pub fn p3_to_srgb() -> [[f64; 3]; 3] {
            let srgb = to_xyz(
                [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]],
                [0.3127, 0.3290],
            );
            let p3 = to_xyz(
                [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
                [0.3127, 0.3290],
            );
            let inv = inverse(srgb);
            let mut out = [[0.0; 3]; 3];
            for (row, out_row) in out.iter_mut().enumerate() {
                for (col, cell) in out_row.iter_mut().enumerate() {
                    *cell = (0..3).map(|k| inv[row][k] * p3[k][col]).sum::<f64>();
                }
            }
            out
        }
    }

    #[test]
    fn the_change_of_primaries_is_what_the_chromaticities_give() {
        let derived = derivation::p3_to_srgb();
        for row in 0..3 {
            for col in 0..3 {
                let stated = P3_TO_SRGB[row][col] as f64;
                assert!(
                    (stated - derived[row][col]).abs() < 1e-6,
                    "row {row} column {col}: source has {stated}, the primaries give {}",
                    derived[row][col]
                );
            }
        }
    }

    /// White is the one color both spaces must agree about, because they share
    /// a white point. Equivalent to every row summing to one, and it catches a
    /// matrix that is scaled or built against the wrong white.
    #[test]
    fn white_is_the_same_color_in_both_spaces() {
        let white = Color::display_p3(1.0, 1.0, 1.0, 1.0).to_array();
        for (component, value) in white.iter().enumerate() {
            assert!(
                close(*value, 1.0),
                "component {component} of P3 white came back {value}"
            );
        }
    }

    /// The two spaces share their blue primary, so pure blue moves along one
    /// axis only. Catches a transposed matrix, which white would not.
    #[test]
    fn the_shared_blue_primary_stays_on_its_own_axis() {
        let [r, g, b, _] = Color::display_p3(0.0, 0.0, 1.0, 1.0).to_array();
        assert_eq!((r, g), (0.0, 0.0), "blue picked up another axis");
        assert!(
            b > 1.0,
            "P3 blue is the same chromaticity at more light: {b}"
        );
    }

    /// The neutral axis is shared, so a grey is a grey in either space. Needs
    /// no derivation to believe.
    #[test]
    fn greys_agree_between_the_spaces() {
        for value in [0.0, 0.25, 0.5, 1.0] {
            let through_srgb = Color::srgb(value, value, value, 1.0).to_array();
            let through_p3 = Color::display_p3(value, value, value, 1.0).to_array();
            for i in 0..4 {
                assert!(
                    close(through_srgb[i], through_p3[i]),
                    "grey {value}, component {i}: {} against {}",
                    through_srgb[i],
                    through_p3[i]
                );
            }
        }
    }

    /// The headline: a saturated P3 red has no representation inside the sRGB
    /// primaries, and saying so takes components below zero.
    #[test]
    fn a_display_p3_red_leaves_the_srgb_primaries() {
        let [r, g, b, a] = Color::display_p3(1.0, 0.0, 0.0, 1.0).to_array();
        assert!(close(r, 1.224_940_2), "{r}");
        assert!(close(g, -0.042_056_95), "{g}");
        assert!(close(b, -0.019_637_55), "{b}");
        assert_eq!(a, 1.0);
        // And re-encoded, which is the form a caller reads and the one the odd
        // extension of the transfer function decides.
        let [er, eg, eb, _] = Color::display_p3(1.0, 0.0, 0.0, 1.0).to_srgb();
        assert!(close(er, 1.093_066), "{er}");
        assert!(close(eg, -0.226_742), "{eg}");
        assert!(close(eb, -0.150_137), "{eb}");
    }

    /// A change of primaries is invertible, so a round trip is exact -- and a
    /// color inside sRGB survives being restated in a wider space and back.
    #[test]
    fn restating_a_color_in_another_space_and_back_returns_it() {
        let original = Color::srgb(0.2, 0.7, 0.4, 0.5);
        let round_tripped = original
            .with_space(ColorSpace::DisplayP3)
            .with_space(ColorSpace::Srgb);
        assert_eq!(round_tripped.space(), ColorSpace::Srgb);
        for (a, b) in original.to_array().iter().zip(round_tripped.to_array()) {
            assert!(close(*a, b), "{a} became {b}");
        }
    }

    /// The two sRGB spaces name the same primaries, so nothing moves between
    /// them. Stated because the alternative -- a matrix that happens to be the
    /// identity -- would look the same until it did not.
    #[test]
    fn the_two_srgb_spaces_describe_the_same_primaries() {
        let wide = Color::linear(1.5, -0.2, 0.3, 1.0).with_space(ColorSpace::ExtendedSrgb);
        assert_eq!(wide.to_array(), [1.5, -0.2, 0.3, 1.0]);
    }
}
