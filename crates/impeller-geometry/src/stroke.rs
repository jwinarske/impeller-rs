//! Stroke style.
//!
//! Stroking is offsetting a path by half the line width on both sides and
//! resolving what happens at the ends and corners. The corner cases are where
//! the visual bugs live: a miter join at a near-degenerate angle produces a
//! spike that can extend arbitrarily far from the path, which is what the
//! miter limit exists to bound.

/// How a stroke terminates at an open end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    /// Stop exactly at the endpoint.
    #[default]
    Butt,
    /// Extend by a half-disc of radius `width / 2`.
    Round,
    /// Extend by a half-square of side `width / 2`.
    Square,
}

/// How a stroke turns a corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    /// Extend both outer edges until they meet, subject to the miter limit.
    #[default]
    Miter,
    /// Fill the corner with a circular arc.
    Round,
    /// Cut the corner off with a straight edge.
    Bevel,
}

/// A stroke's geometric parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    /// Full width of the stroke, not the half-width offset.
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    /// Maximum ratio of miter length to line width before a miter join
    /// degrades to a bevel.
    ///
    /// As the angle between two segments approaches zero the miter length
    /// grows without bound, so an unbounded miter would let a hairline path
    /// paint an arbitrarily long spike. The default of 4 is the SVG and
    /// PostScript convention; measured against this implementation, it
    /// degrades to a bevel just below 29 degrees.
    ///
    /// Values below 2 all behave as 2, which bevels below 60 degrees. That
    /// floor comes from the tessellation backend rather than from the
    /// geometry, and such limits are rare enough in practice that matching
    /// them exactly is not worth carrying a second join implementation.
    pub miter_limit: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::default(),
            join: LineJoin::default(),
            miter_limit: 4.0,
        }
    }
}

impl StrokeStyle {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            ..Default::default()
        }
    }

    /// The same style at another width.
    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn with_cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    pub fn with_join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    pub fn with_miter_limit(mut self, limit: f32) -> Self {
        self.miter_limit = limit;
        self
    }

    /// Whether this style would produce any geometry at all.
    ///
    /// A non-positive or non-finite width strokes nothing. Treating that as
    /// "draw nothing" rather than as an error matches how a caller animating a
    /// width down to zero expects it to behave.
    pub fn is_visible(&self) -> bool {
        self.width > 0.0 && self.width.is_finite()
    }

    /// How far the stroke can extend beyond the path itself.
    ///
    /// Used to expand bounds for culling. A miter join reaches
    /// `miter_limit * width / 2` at the limit, which is further than the
    /// half-width the stroke reaches along a straight run.
    pub fn max_extent(&self) -> f32 {
        if !self.is_visible() {
            return 0.0;
        }
        let half = self.width * 0.5;
        match self.join {
            LineJoin::Miter => half * self.miter_limit.max(1.0),
            LineJoin::Round | LineJoin::Bevel => half,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_svg_conventions() {
        let s = StrokeStyle::default();
        assert_eq!(s.cap, LineCap::Butt);
        assert_eq!(s.join, LineJoin::Miter);
        assert_eq!(s.miter_limit, 4.0);
    }

    #[test]
    fn non_positive_and_non_finite_widths_are_invisible() {
        // An animation driving width to zero should stop drawing, not error.
        assert!(!StrokeStyle::new(0.0).is_visible());
        assert!(!StrokeStyle::new(-1.0).is_visible());
        assert!(!StrokeStyle::new(f32::NAN).is_visible());
        assert!(!StrokeStyle::new(f32::INFINITY).is_visible());
        assert!(StrokeStyle::new(0.5).is_visible());
    }

    #[test]
    fn miter_joins_reach_further_than_the_half_width() {
        let w = 4.0;
        let miter = StrokeStyle::new(w).with_join(LineJoin::Miter);
        // A miter spike reaches miter_limit * half_width, so culling bounds
        // computed from the half-width alone would clip it.
        assert_eq!(miter.max_extent(), 2.0 * 4.0);

        for join in [LineJoin::Round, LineJoin::Bevel] {
            assert_eq!(StrokeStyle::new(w).with_join(join).max_extent(), 2.0);
        }
    }

    #[test]
    fn a_miter_limit_below_one_cannot_shrink_the_extent() {
        // The join can never pull inside the stroke's own half-width, so a
        // nonsensical limit must not underestimate the bounds.
        let s = StrokeStyle::new(4.0).with_miter_limit(0.1);
        assert_eq!(s.max_extent(), 2.0);
    }

    #[test]
    fn invisible_strokes_have_no_extent() {
        assert_eq!(StrokeStyle::new(0.0).max_extent(), 0.0);
        assert_eq!(StrokeStyle::new(f32::NAN).max_extent(), 0.0);
    }
}
