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

use glam::{Affine2, Vec2};

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

/// Apply a transform to every point in place.
pub fn transform_points(points: &mut [Vec2], transform: &Affine2) {
    for p in points.iter_mut() {
        *p = transform.transform_point2(*p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
