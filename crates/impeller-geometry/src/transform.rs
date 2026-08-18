//! Mapping user space onto clip space.
//!
//! Three coordinate systems meet here, and confusing any two of them produces
//! output that is mirrored or off by a scale factor rather than obviously
//! broken:
//!
//! - **Path space** — whatever units a path was authored in.
//! - **Device space** — pixels on the target. Origin top-left, X right, Y
//!   *down*, which is the convention 2D UI works in.
//! - **Clip space** — what the vertex shader emits. Both axes in `[-1, 1]`,
//!   and Y runs *up*, following the WGSL convention that shader translation
//!   normalizes to. This is the axis that flips.
//!
//! Tolerance is a device-space quantity, so flattening a path that will be
//! scaled up has to happen more finely in path space. [`max_scale`] is what
//! that adjustment is computed from.

use glam::{Affine2, Mat2, Vec2};

/// Map device pixels onto clip space for a target of the given size.
///
/// Device Y runs down from the top-left and clip Y runs up, so this flips the
/// Y axis. Getting the flip wrong renders everything upside down, which is
/// easy to miss on symmetric test content and obvious on text.
pub fn viewport_projection(width: u32, height: u32) -> Affine2 {
    // Guard against a zero-sized target producing infinities that then
    // propagate into every vertex as NaN.
    let w = if width == 0 { 1.0 } else { width as f32 };
    let h = if height == 0 { 1.0 } else { height as f32 };
    Affine2::from_cols(
        Vec2::new(2.0 / w, 0.0),
        Vec2::new(0.0, -2.0 / h),
        Vec2::new(-1.0, 1.0),
    )
}

/// The largest factor by which a transform can stretch a direction.
///
/// Used to scale flattening tolerance: a curve drawn at four times its
/// authored size needs finer subdivision in path space to stay within the same
/// device-space error. Estimated from the basis vector lengths, which bounds
/// the true largest singular value from below by at most a factor of the
/// square root of two — close enough for choosing a segment count, and cheaper
/// than a decomposition.
pub fn max_scale(transform: &Affine2) -> f32 {
    let x = transform.matrix2.x_axis.length();
    let y = transform.matrix2.y_axis.length();
    x.max(y)
}

/// Whether a transform maps axis-aligned rectangles to axis-aligned rectangles.
///
/// True for any composition of translation, scale, reflection, and quarter
/// turns; false as soon as an arbitrary rotation or a skew is involved. This is
/// what decides whether a rectangular clip can be handed to the fixed-function
/// scissor unit exactly, or whether it needs machinery that can express a
/// rotated quadrilateral.
///
/// The comparison is relative rather than exact because a quarter turn does not
/// produce exact zeros: `cos` of a right angle in `f32` is about `-4.4e-8`, so
/// a caller who asked for exactly that rotation would otherwise be told their
/// rectangle is no longer one. The threshold is scaled by the transform's own
/// magnitude, since an absolute one means something different at a scale of a
/// thousand than at a scale of a thousandth.
pub fn preserves_axis_alignment(transform: &Affine2) -> bool {
    let m = transform.matrix2;
    let magnitude = max_scale(transform);
    if magnitude == 0.0 || !magnitude.is_finite() {
        // A degenerate transform collapses every rectangle to a line or a
        // point. That is a rectangle in the trivial sense and an empty clip in
        // practice, so it is left to the caller's intersection to resolve
        // rather than reported as an unsupported shape.
        return true;
    }
    let epsilon = 1e-6 * magnitude;
    // Diagonal is a scale, possibly reflected; anti-diagonal is that composed
    // with a quarter turn. Anything else rotates or skews.
    let diagonal = m.x_axis.y.abs() <= epsilon && m.y_axis.x.abs() <= epsilon;
    let anti_diagonal = m.x_axis.x.abs() <= epsilon && m.y_axis.y.abs() <= epsilon;
    diagonal || anti_diagonal
}

/// The bounds of a rectangle's corners after a transform.
///
/// Exact when [`preserves_axis_alignment`] holds, and the bounding box of a
/// rotated quadrilateral otherwise — which is why callers that need the clip to
/// be the region asked for must check that first rather than relying on this to
/// tell them.
pub fn transformed_bounds(transform: &Affine2, min: Vec2, max: Vec2) -> (Vec2, Vec2) {
    let corners = [
        transform.transform_point2(min),
        transform.transform_point2(Vec2::new(max.x, min.y)),
        transform.transform_point2(max),
        transform.transform_point2(Vec2::new(min.x, max.y)),
    ];
    corners.iter().fold(
        (corners[0], corners[0]),
        |(lo, hi): (Vec2, Vec2), c: &Vec2| (lo.min(*c), hi.max(*c)),
    )
}

/// Apply a transform to every point in place.
pub fn transform_points(points: &mut [Vec2], transform: &Affine2) {
    for p in points.iter_mut() {
        *p = transform.transform_point2(*p);
    }
}

/// Invert a mapping's linear part, falling back to the identity.
///
/// The result is in the column-major four-float form a shader's `to_local`
/// takes, which is what every caller wants it for: a paint states its geometry
/// in one space and the fragment stage arrives in another, so it carries the
/// mapping between them.
///
/// A degenerate transform — a zero scale, or one axis collapsed — has no
/// inverse. That is a caller mistake rather than a renderer one, and the shape
/// it fills is collapsed to nothing anyway, so the paint it would have carried
/// is not observable. Returning the identity keeps a non-finite matrix out of
/// the shader, where it would spread NaN across every pixel of the draw.
pub fn invert_or_identity(matrix: Mat2) -> [f32; 4] {
    let determinant = matrix.determinant();
    let inverse = if determinant.abs() > 1e-9 && determinant.is_finite() {
        matrix.inverse()
    } else {
        Mat2::IDENTITY
    };
    let columns = inverse.to_cols_array();
    if columns.iter().all(|v| v.is_finite()) {
        columns
    } else {
        Mat2::IDENTITY.to_cols_array()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_translations_and_quarter_turns_keep_rectangles_rectangular() {
        let quarter = std::f32::consts::FRAC_PI_2;
        for transform in [
            Affine2::IDENTITY,
            Affine2::from_translation(Vec2::new(13.0, -4.0)),
            Affine2::from_scale(Vec2::new(3.0, 0.5)),
            // A reflection, which is a negative scale rather than a rotation.
            Affine2::from_scale(Vec2::new(-1.0, 1.0)),
            Affine2::from_angle(quarter),
            Affine2::from_angle(-quarter),
            Affine2::from_angle(2.0 * quarter),
            // Composition of all of them is still axis-preserving.
            Affine2::from_angle(quarter)
                * Affine2::from_scale(Vec2::new(2.0, 7.0))
                * Affine2::from_translation(Vec2::new(1.0, 1.0)),
        ] {
            assert!(
                preserves_axis_alignment(&transform),
                "{transform:?} was rejected"
            );
        }
    }

    #[test]
    fn arbitrary_rotations_and_skews_do_not() {
        let mut skew = Affine2::IDENTITY;
        skew.matrix2.y_axis.x = 0.4;
        for transform in [
            Affine2::from_angle(0.3),
            Affine2::from_angle(std::f32::consts::FRAC_PI_4),
            skew,
        ] {
            assert!(
                !preserves_axis_alignment(&transform),
                "{transform:?} was accepted"
            );
        }
    }

    #[test]
    fn the_threshold_scales_with_the_transform() {
        // The same tiny skew is noise beside a large scale and the whole of the
        // transform beside a small one. An absolute threshold would call both
        // the same thing.
        let skew = 1e-3;
        let mut large = Affine2::from_scale(Vec2::splat(1e4));
        large.matrix2.y_axis.x = skew;
        assert!(preserves_axis_alignment(&large));

        let mut small = Affine2::from_scale(Vec2::splat(1e-2));
        small.matrix2.y_axis.x = skew;
        assert!(!preserves_axis_alignment(&small));
    }

    #[test]
    fn a_quarter_turn_maps_a_rectangle_onto_the_other_axis() {
        let quarter = Affine2::from_angle(std::f32::consts::FRAC_PI_2);
        let (min, max) = transformed_bounds(&quarter, Vec2::new(0.0, 0.0), Vec2::new(4.0, 1.0));
        // Width and height exchange places; the corner positions follow the
        // rotation rather than staying put.
        assert!(
            (max.x - min.x - 1.0).abs() < 1e-5,
            "width became {}",
            max.x - min.x
        );
        assert!(
            (max.y - min.y - 4.0).abs() < 1e-5,
            "height became {}",
            max.y - min.y
        );
    }

    fn approx(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn the_top_left_pixel_maps_to_the_top_of_clip_space() {
        let p = viewport_projection(64, 64);
        // Device origin is top-left; clip Y runs up, so the top is +1.
        assert!(approx(
            p.transform_point2(Vec2::new(0.0, 0.0)),
            Vec2::new(-1.0, 1.0)
        ));
    }

    #[test]
    fn the_bottom_right_corner_maps_to_the_bottom_of_clip_space() {
        let p = viewport_projection(64, 64);
        assert!(approx(
            p.transform_point2(Vec2::new(64.0, 64.0)),
            Vec2::new(1.0, -1.0)
        ));
    }

    #[test]
    fn the_center_maps_to_the_origin() {
        let p = viewport_projection(800, 600);
        assert!(approx(
            p.transform_point2(Vec2::new(400.0, 300.0)),
            Vec2::ZERO
        ));
    }

    #[test]
    fn the_y_axis_is_flipped_and_the_x_axis_is_not() {
        let p = viewport_projection(100, 100);
        let top = p.transform_point2(Vec2::new(50.0, 10.0));
        let bottom = p.transform_point2(Vec2::new(50.0, 90.0));
        // Further down in device space must be further down in clip space,
        // which under a Y-up clip convention means a smaller value.
        assert!(top.y > bottom.y, "device Y down should map to clip Y up");

        let left = p.transform_point2(Vec2::new(10.0, 50.0));
        let right = p.transform_point2(Vec2::new(90.0, 50.0));
        assert!(left.x < right.x, "X must not be flipped");
    }

    #[test]
    fn a_non_square_target_scales_each_axis_independently() {
        let p = viewport_projection(200, 100);
        // Half the width in device space is the same clip distance as half the
        // height; a single shared scale factor would squash one axis.
        assert!(approx(
            p.transform_point2(Vec2::new(200.0, 0.0)),
            Vec2::new(1.0, 1.0)
        ));
        assert!(approx(
            p.transform_point2(Vec2::new(0.0, 100.0)),
            Vec2::new(-1.0, -1.0)
        ));
    }

    #[test]
    fn a_zero_sized_target_does_not_produce_non_finite_coordinates() {
        let p = viewport_projection(0, 0);
        let v = p.transform_point2(Vec2::new(1.0, 1.0));
        assert!(v.is_finite(), "got {v:?}");
    }

    #[test]
    fn max_scale_reports_the_larger_axis() {
        let t = Affine2::from_scale(Vec2::new(3.0, 7.0));
        assert!((max_scale(&t) - 7.0).abs() < 1e-5);
        assert!((max_scale(&Affine2::IDENTITY) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn max_scale_is_unchanged_by_rotation_and_translation() {
        let rotated = Affine2::from_angle(0.9) * Affine2::from_scale(Vec2::splat(2.0));
        assert!((max_scale(&rotated) - 2.0).abs() < 1e-4);

        let translated = Affine2::from_translation(Vec2::new(100.0, -50.0));
        assert!((max_scale(&translated) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn transforming_points_matches_transforming_them_one_at_a_time() {
        let t =
            Affine2::from_scale_angle_translation(Vec2::new(2.0, 3.0), 0.4, Vec2::new(5.0, -1.0));
        let source = [Vec2::new(1.0, 2.0), Vec2::new(-3.0, 4.0), Vec2::ZERO];
        let mut batch = source;
        transform_points(&mut batch, &t);
        for (i, p) in source.iter().enumerate() {
            assert!(approx(batch[i], t.transform_point2(*p)));
        }
    }
}
