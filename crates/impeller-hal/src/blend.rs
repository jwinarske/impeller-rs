//! How a draw combines with what a target already holds.
//!
//! Everything here is expressible with fixed-function blend factors, so it
//! works on every device with no extension and no capability gate. That is why
//! this set comes first: the separable and non-separable modes — multiply,
//! screen, overlay, hue and the rest — need an advanced-blend extension, and a
//! renderer that could not composite without one would be unusable on the
//! hardware least likely to have it.
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
}

impl BlendMode {
    /// Every mode, for exhaustive tests and reporting.
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
    ];

    /// The factors this mode blends with, assuming premultiplied colour.
    pub const fn factors(self) -> BlendFactors {
        use BlendFactor::*;
        match self {
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
        }
    }

    /// Whether the destination contributes to the result.
    ///
    /// A mode that ignores it can skip loading the target on a tiler, which is
    /// a bandwidth saving rather than a correctness one.
    pub const fn reads_destination(self) -> bool {
        !matches!(self.factors().dst, BlendFactor::Zero)
            || matches!(
                self.factors().src,
                BlendFactor::DstAlpha | BlendFactor::OneMinusDstAlpha | BlendFactor::DstColor
            )
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
        }
    }
}

impl std::fmt::Display for BlendMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
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
        let f = mode.factors();
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
        for mode in BlendMode::ALL {
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
    fn every_mode_has_a_distinct_name() {
        let mut names: Vec<&str> = BlendMode::ALL.iter().map(|m| m.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate blend mode name");
    }
}
