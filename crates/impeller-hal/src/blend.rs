//! How a draw combines with what a target already holds.
//!
//! Two families live here, and they differ in kind rather than degree.
//!
//! The Porter-Duff modes decide *where* each side survives, and are expressible
//! with fixed-function blend factors: they work on every device with no
//! extension and no capability gate, which is why they came first. A renderer
//! unable to composite without an extension would be unusable on the hardware
//! least likely to have one.
//!
//! The separable modes — multiply, screen, overlay and the rest — mix the two
//! sides *arithmetically*, channel by channel. No combination of blend factors
//! expresses that, so they need an advanced-blend extension and are
//! capability-gated. Their formulas are fixed by the specification, which is
//! what makes them testable against a computed expectation rather than against
//! a recorded picture.
//!
//! Every mode assumes **premultiplied** colour, which is what the render target
//! holds. The factors differ from the straight-alpha forms: source-over is
//! `ONE` rather than `SRC_ALPHA`, because the source has already been scaled.

/// A factor a blend equation multiplies one side by.
///
/// Portable rather than each backend naming its own, so the Porter-Duff table
/// below exists once instead of once per backend, where the two copies would
/// drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlendFactor {
    Zero,
    One,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    DstColor,
}

/// The factors for one blend mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendFactors {
    pub src: BlendFactor,
    pub dst: BlendFactor,
}

impl BlendFactors {
    const fn new(src: BlendFactor, dst: BlendFactor) -> Self {
        Self { src, dst }
    }
}

/// How a draw combines with what a target already holds.
///
/// The Porter-Duff set, which covers compositing: which of the source and
/// destination survive, and where. Modes that mix colour channels arithmetically
/// arrive with advanced blending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    /// Leave nothing behind. Neither source nor destination survives.
    Clear,
    /// Replace the destination outright, alpha included.
    ///
    /// Not the same as [`Self::SrcOver`] with an opaque source: this overwrites
    /// destination alpha too, which decides whether a layer composites
    /// correctly when it is itself drawn onto something else.
    Src,
    /// Keep the destination and discard the source.
    Dst,
    /// Source over destination: the default for drawing one thing on another.
    #[default]
    SrcOver,
    /// Destination over source, as though the source were drawn underneath.
    DstOver,
    /// Source, clipped to where the destination is.
    SrcIn,
    /// Destination, clipped to where the source is.
    DstIn,
    /// Source, clipped to where the destination is not.
    SrcOut,
    /// Destination, clipped to where the source is not.
    DstOut,
    /// Source drawn on top, but only within the destination's shape.
    SrcATop,
    /// Destination drawn on top, but only within the source's shape.
    DstATop,
    /// Whichever of the two is not overlapped by the other.
    Xor,
    /// The two added together, saturating.
    ///
    /// Used for light accumulation, where overlapping contributions should
    /// brighten rather than replace.
    Plus,
    /// The two multiplied together.
    Modulate,

    // Everything below mixes the two sides arithmetically and requires
    // advanced blending. The formulas are the separable ones from the
    // compositing specification, applied per channel to unpremultiplied colour.
    /// Multiply the two, which always darkens.
    Multiply,
    /// The inverse of multiplying the inverses, which always lightens.
    Screen,
    /// Multiply or screen depending on the destination, so the destination
    /// decides the contrast.
    Overlay,
    /// The darker of the two, per channel.
    Darken,
    /// The lighter of the two, per channel.
    Lighten,
    /// Brighten the destination in proportion to the source.
    ColorDodge,
    /// Darken the destination in proportion to the inverse of the source.
    ColorBurn,
    /// Overlay with the roles reversed, so the source decides the contrast.
    HardLight,
    /// A gentler hard-light, without the hard transition at the midpoint.
    SoftLight,
    /// The absolute difference, which inverts where the two agree.
    Difference,
    /// Like difference, but with a softer response near the midpoint.
    Exclusion,
}

impl BlendMode {
    /// The modes every device can do, with no extension.
    pub const PORTER_DUFF: &'static [Self] = &[
        Self::Clear,
        Self::Src,
        Self::Dst,
        Self::SrcOver,
        Self::DstOver,
        Self::SrcIn,
        Self::DstIn,
        Self::SrcOut,
        Self::DstOut,
        Self::SrcATop,
        Self::DstATop,
        Self::Xor,
        Self::Plus,
        Self::Modulate,
    ];

    /// The modes that need advanced blending.
    pub const ADVANCED: &'static [Self] = &[
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
        Self::ColorDodge,
        Self::ColorBurn,
        Self::HardLight,
        Self::SoftLight,
        Self::Difference,
        Self::Exclusion,
    ];

    /// Every mode, for exhaustive tests and reporting.
    ///
    /// Deliberately *not* what a backend iterates to decide what it supports —
    /// that is what [`Self::is_advanced`] and the capability flag are for.
    pub const ALL: &'static [Self] = &[
        Self::Clear,
        Self::Src,
        Self::Dst,
        Self::SrcOver,
        Self::DstOver,
        Self::SrcIn,
        Self::DstIn,
        Self::SrcOut,
        Self::DstOut,
        Self::SrcATop,
        Self::DstATop,
        Self::Xor,
        Self::Plus,
        Self::Modulate,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
        Self::ColorDodge,
        Self::ColorBurn,
        Self::HardLight,
        Self::SoftLight,
        Self::Difference,
        Self::Exclusion,
    ];

    /// Whether this mode needs advanced blending.
    ///
    /// Callers check [`Capabilities::advanced_blend`] before using one, and a
    /// backend without it refuses rather than substituting something that looks
    /// close: a silently wrong blend mode is a picture nobody can debug from.
    pub const fn is_advanced(self) -> bool {
        !matches!(
            self,
            Self::Clear
                | Self::Src
                | Self::Dst
                | Self::SrcOver
                | Self::DstOver
                | Self::SrcIn
                | Self::DstIn
                | Self::SrcOut
                | Self::DstOut
                | Self::SrcATop
                | Self::DstATop
                | Self::Xor
                | Self::Plus
                | Self::Modulate
        )
    }

    /// The factors this mode blends with, assuming premultiplied colour.
    ///
    /// `None` for an advanced mode: those are not expressible as factors at
    /// all, which is exactly why they need an extension. Returning an
    /// option rather than a plausible pair keeps a backend from silently
    /// rendering the wrong thing.
    pub const fn factors(self) -> Option<BlendFactors> {
        use BlendFactor::*;
        Some(match self {
            Self::Clear => BlendFactors::new(Zero, Zero),
            Self::Src => BlendFactors::new(One, Zero),
            Self::Dst => BlendFactors::new(Zero, One),
            Self::SrcOver => BlendFactors::new(One, OneMinusSrcAlpha),
            Self::DstOver => BlendFactors::new(OneMinusDstAlpha, One),
            Self::SrcIn => BlendFactors::new(DstAlpha, Zero),
            Self::DstIn => BlendFactors::new(Zero, SrcAlpha),
            Self::SrcOut => BlendFactors::new(OneMinusDstAlpha, Zero),
            Self::DstOut => BlendFactors::new(Zero, OneMinusSrcAlpha),
            Self::SrcATop => BlendFactors::new(DstAlpha, OneMinusSrcAlpha),
            Self::DstATop => BlendFactors::new(OneMinusDstAlpha, SrcAlpha),
            Self::Xor => BlendFactors::new(OneMinusDstAlpha, OneMinusSrcAlpha),
            Self::Plus => BlendFactors::new(One, One),
            Self::Modulate => BlendFactors::new(DstColor, Zero),
            _ => return None,
        })
    }

    /// Whether the destination contributes to the result.
    ///
    /// A mode that ignores it can skip loading the target on a tiler, which is
    /// a bandwidth saving rather than a correctness one.
    pub const fn reads_destination(self) -> bool {
        match self.factors() {
            Some(factors) => {
                !matches!(factors.dst, BlendFactor::Zero)
                    || matches!(
                        factors.src,
                        BlendFactor::DstAlpha
                            | BlendFactor::OneMinusDstAlpha
                            | BlendFactor::DstColor
                    )
            }
            // Every advanced mode is a function of both sides by definition.
            None => true,
        }
    }

    /// Whether this simply writes the source, so blending can be switched off.
    pub const fn is_plain_write(self) -> bool {
        matches!(self, Self::Src)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Src => "src",
            Self::Dst => "dst",
            Self::SrcOver => "src-over",
            Self::DstOver => "dst-over",
            Self::SrcIn => "src-in",
            Self::DstIn => "dst-in",
            Self::SrcOut => "src-out",
            Self::DstOut => "dst-out",
            Self::SrcATop => "src-atop",
            Self::DstATop => "dst-atop",
            Self::Xor => "xor",
            Self::Plus => "plus",
            Self::Modulate => "modulate",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
            Self::ColorDodge => "color-dodge",
            Self::ColorBurn => "color-burn",
            Self::HardLight => "hard-light",
            Self::SoftLight => "soft-light",
            Self::Difference => "difference",
            Self::Exclusion => "exclusion",
        }
    }
}

impl std::fmt::Display for BlendMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The separable blend function `B(Cb, Cs)`, on one unpremultiplied channel.
///
/// This is the compositing specification's definition transcribed directly.
/// It exists so there is exactly one statement of what each mode means: the
/// conformance tests check the hardware against it, and any future software
/// path evaluates it. Nothing here is a second implementation of the backend
/// mapping — that maps an enum to an extension's blend op and shares no code
/// with this, which is what makes checking one against the other meaningful.
///
/// Returns `None` for a mode that is not separable-advanced.
pub fn separable_blend(mode: BlendMode, backdrop: f32, source: f32) -> Option<f32> {
    let (cb, cs) = (backdrop, source);
    let multiply = |a: f32, b: f32| a * b;
    let screen = |a: f32, b: f32| a + b - a * b;
    // Hard-light is its own function *and* the body of overlay with the two
    // sides exchanged, so it is written once and called twice.
    let hard_light = |cb: f32, cs: f32| {
        if cs <= 0.5 {
            multiply(cb, 2.0 * cs)
        } else {
            screen(cb, 2.0 * cs - 1.0)
        }
    };
    Some(match mode {
        BlendMode::Multiply => multiply(cb, cs),
        BlendMode::Screen => screen(cb, cs),
        BlendMode::Overlay => hard_light(cs, cb),
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::ColorDodge => {
            // The order of these three cases is load-bearing: a black backdrop
            // stays black even under a full-strength source, and only then does
            // a full-strength source saturate. Swapping them makes the corner
            // where both hold produce white instead of black.
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        BlendMode::HardLight => hard_light(cb, cs),
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                // The specification's D(Cb): a cubic below a quarter, a square
                // root above it, chosen so the two meet with equal slope.
                let d = if cb <= 0.25 {
                    ((16.0 * cb - 12.0) * cb + 4.0) * cb
                } else {
                    cb.sqrt()
                };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        BlendMode::Difference => (cs - cb).abs(),
        BlendMode::Exclusion => cs + cb - 2.0 * cs * cb,
        _ => return None,
    })
}

/// An advanced mode applied to premultiplied colors, giving premultiplied color.
///
/// The blend function itself is defined on unpremultiplied channels, so this
/// un-premultiplies, blends, and recombines using the specification's
/// composite: the blended color applies only where the two sides overlap, and
/// each side survives alone where the other is absent.
pub fn blend_advanced(mode: BlendMode, source: [f32; 4], backdrop: [f32; 4]) -> Option<[f32; 4]> {
    let (a_s, a_b) = (source[3], backdrop[3]);
    let mut out = [0.0f32; 4];
    out[3] = a_s + a_b - a_s * a_b;
    for channel in 0..3 {
        // Dividing by a zero alpha would give a NaN that then propagates
        // through a term the same alpha multiplies away, so the color under a
        // fully transparent side is taken as zero rather than computed.
        let cs = if a_s > 0.0 {
            source[channel] / a_s
        } else {
            0.0
        };
        let cb = if a_b > 0.0 {
            backdrop[channel] / a_b
        } else {
            0.0
        };
        let blended = separable_blend(mode, cb, cs)?;
        out[channel] = a_s * (1.0 - a_b) * cs + a_s * a_b * blended + (1.0 - a_s) * a_b * cb;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate a factor against premultiplied source and destination.
    fn value(factor: BlendFactor, src: [f32; 4], dst: [f32; 4], channel: usize) -> f32 {
        match factor {
            BlendFactor::Zero => 0.0,
            BlendFactor::One => 1.0,
            BlendFactor::SrcAlpha => src[3],
            BlendFactor::OneMinusSrcAlpha => 1.0 - src[3],
            BlendFactor::DstAlpha => dst[3],
            BlendFactor::OneMinusDstAlpha => 1.0 - dst[3],
            BlendFactor::DstColor => dst[channel],
        }
    }

    /// The blend equation, as the hardware applies it.
    fn blend(mode: BlendMode, src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
        let f = mode.factors().expect("Porter-Duff mode");
        let mut out = [0.0f32; 4];
        for channel in 0..4 {
            out[channel] = (src[channel] * value(f.src, src, dst, channel)
                + dst[channel] * value(f.dst, src, dst, channel))
            .clamp(0.0, 1.0);
        }
        out
    }

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    // Premultiplied half-opaque red and opaque blue.
    const SRC: [f32; 4] = [0.5, 0.0, 0.0, 0.5];
    const DST: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    #[test]
    fn clear_leaves_nothing() {
        assert!(close(blend(BlendMode::Clear, SRC, DST), [0.0; 4]));
    }

    #[test]
    fn src_and_dst_each_keep_one_side_entirely() {
        assert!(close(blend(BlendMode::Src, SRC, DST), SRC));
        assert!(close(blend(BlendMode::Dst, SRC, DST), DST));
    }

    #[test]
    fn source_over_and_destination_over_are_mirror_images() {
        // Half-opaque red over opaque blue: half the blue survives.
        assert!(close(
            blend(BlendMode::SrcOver, SRC, DST),
            [0.5, 0.0, 0.5, 1.0]
        ));
        // The other way round, the destination is opaque so nothing shows
        // through and the source is entirely hidden.
        assert!(close(blend(BlendMode::DstOver, SRC, DST), DST));
    }

    #[test]
    fn the_in_modes_clip_one_side_to_the_other() {
        // Source clipped to an opaque destination is the source unchanged.
        assert!(close(blend(BlendMode::SrcIn, SRC, DST), SRC));
        // Destination clipped to a half-opaque source keeps half of it.
        assert!(close(
            blend(BlendMode::DstIn, SRC, DST),
            [0.0, 0.0, 0.5, 0.5]
        ));
    }

    #[test]
    fn the_out_modes_are_the_complement_of_the_in_modes() {
        // The destination is opaque, so there is nowhere the source is outside
        // it: source-out leaves nothing.
        assert!(close(blend(BlendMode::SrcOut, SRC, DST), [0.0; 4]));
        // Half the destination lies outside a half-opaque source.
        assert!(close(
            blend(BlendMode::DstOut, SRC, DST),
            [0.0, 0.0, 0.5, 0.5]
        ));
    }

    #[test]
    fn atop_keeps_the_shape_of_the_side_it_is_named_for() {
        // Source atop destination has the destination's shape: alpha stays at
        // the destination's, which is what distinguishes it from source-over.
        let result = blend(BlendMode::SrcATop, SRC, DST);
        assert!((result[3] - DST[3]).abs() < 1e-5, "alpha should follow dst");
        assert!(close(result, [0.5, 0.0, 0.5, 1.0]));
    }

    #[test]
    fn xor_keeps_only_what_the_other_side_does_not_cover() {
        // An opaque destination covers everything, so only the part of the
        // destination outside the half-opaque source survives.
        assert!(close(blend(BlendMode::Xor, SRC, DST), [0.0, 0.0, 0.5, 0.5]));
    }

    #[test]
    fn plus_accumulates_and_saturates() {
        let sum = blend(BlendMode::Plus, SRC, DST);
        assert!(close(sum, [0.5, 0.0, 1.0, 1.0]));
        // Saturating rather than wrapping: two bright sources must not go dark.
        let bright = blend(BlendMode::Plus, [0.8, 0.8, 0.8, 1.0], [0.8, 0.8, 0.8, 1.0]);
        assert!(close(bright, [1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn modulate_multiplies_the_two() {
        let result = blend(
            BlendMode::Modulate,
            [0.5, 1.0, 0.5, 1.0],
            [0.5, 0.5, 1.0, 1.0],
        );
        assert!(close(result, [0.25, 0.5, 0.5, 1.0]));
    }

    #[test]
    fn only_src_can_skip_blending_entirely() {
        // Everything else needs the hardware to combine two sides, so treating
        // another mode as a plain write would silently drop the destination.
        for mode in BlendMode::PORTER_DUFF {
            assert_eq!(
                mode.is_plain_write(),
                *mode == BlendMode::Src,
                "{mode} misreported whether it is a plain write"
            );
        }
    }

    #[test]
    fn modes_that_ignore_the_destination_are_identified() {
        // A mode that never reads the target can skip loading it on a tiler.
        for mode in [BlendMode::Clear, BlendMode::Src] {
            assert!(!mode.reads_destination(), "{mode}");
        }
        for mode in [
            BlendMode::SrcOver,
            BlendMode::DstOver,
            BlendMode::SrcIn,
            BlendMode::Modulate,
            BlendMode::Xor,
        ] {
            assert!(mode.reads_destination(), "{mode}");
        }
    }

    #[test]
    fn the_separable_modes_hit_their_defining_fixed_points() {
        // Each identity below is what the mode is *for*, so getting one wrong
        // means the transcription is wrong regardless of what the hardware does.
        let cases: &[(BlendMode, f32, f32, f32)] = &[
            (BlendMode::Multiply, 0.5, 0.5, 0.25),
            (BlendMode::Screen, 0.5, 0.5, 0.75),
            (BlendMode::Darken, 0.2, 0.8, 0.2),
            (BlendMode::Lighten, 0.2, 0.8, 0.8),
            (BlendMode::Difference, 0.2, 0.8, 0.6),
            (BlendMode::Exclusion, 0.5, 0.5, 0.5),
            // Half-strength hard-light and overlay are both the identity on the
            // side that decides the contrast.
            (BlendMode::HardLight, 0.3, 0.5, 0.3),
            (BlendMode::Overlay, 0.5, 0.3, 0.3),
            // Half-strength soft-light leaves the backdrop untouched.
            (BlendMode::SoftLight, 0.3, 0.5, 0.3),
            // The dodge and burn corners, where the ordering of the guards shows.
            (BlendMode::ColorDodge, 0.0, 1.0, 0.0),
            (BlendMode::ColorDodge, 0.25, 0.5, 0.5),
            (BlendMode::ColorBurn, 1.0, 0.0, 1.0),
            (BlendMode::ColorBurn, 0.5, 0.5, 0.0),
        ];
        for &(mode, cb, cs, want) in cases {
            let got = separable_blend(mode, cb, cs).expect("separable");
            assert!(
                (got - want).abs() < 1e-6,
                "{mode}(backdrop {cb}, source {cs}) gave {got}, want {want}"
            );
        }
    }

    #[test]
    fn soft_light_is_continuous_where_its_two_branches_meet() {
        // The branches join at a source of one half and at a backdrop of one
        // quarter; a transcription error in either shows as a step.
        for &(cb, cs) in &[(0.25, 0.5), (0.2499, 0.75), (0.5, 0.4999)] {
            let here = separable_blend(BlendMode::SoftLight, cb, cs).unwrap();
            let there = separable_blend(BlendMode::SoftLight, cb + 2e-4, cs + 2e-4).unwrap();
            assert!((here - there).abs() < 1e-3, "step at ({cb}, {cs})");
        }
    }

    #[test]
    fn an_advanced_mode_over_nothing_is_just_the_source() {
        // Where the backdrop is absent the blend function has nothing to mix
        // with, so every mode must reduce to the source unchanged.
        let src = [0.3, 0.0, 0.15, 0.6];
        for mode in BlendMode::ADVANCED {
            let out = blend_advanced(*mode, src, [0.0; 4]).expect("advanced");
            assert!(close(out, src), "{mode} over nothing gave {out:?}");
        }
    }

    #[test]
    fn the_two_families_do_not_overlap() {
        // Every mode belongs to exactly one family, and the family it claims
        // agrees with whether it has factors.
        assert_eq!(
            BlendMode::PORTER_DUFF.len() + BlendMode::ADVANCED.len(),
            BlendMode::ALL.len()
        );
        for mode in BlendMode::ALL {
            assert_eq!(
                mode.is_advanced(),
                mode.factors().is_none(),
                "{mode} disagrees with itself about which family it is in"
            );
            assert_eq!(
                mode.is_advanced(),
                separable_blend(*mode, 0.5, 0.5).is_some(),
                "{mode} has no separable formula but claims to be advanced"
            );
        }
    }

    #[test]
    fn every_mode_has_a_distinct_name() {
        let mut names: Vec<&str> = BlendMode::ALL.iter().map(|m| m.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate blend mode name");
    }
}
