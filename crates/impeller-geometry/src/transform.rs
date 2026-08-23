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

use glam::{Affine2, Mat2, Mat3, Vec2, Vec3};

/// A transform of the plane that may carry perspective.
///
/// A three-by-three homography acting on a point written `(x, y, 1)`, which is
/// what `dart:ui`'s four-by-four reduces to for content on the `z = 0` plane.
/// An affine is the case whose bottom row is `(0, 0, 1)`, and it is the common
/// one — [`is_affine`](Self::is_affine) is how the fast paths ask.
///
/// # Why this is a newtype and not a `Mat3`
///
/// Because [`glam::Mat3::transform_point2`] exists, compiles here, reads
/// correctly, and is wrong. It documents itself as assuming a valid affine
/// transform: it computes `x_axis * x + y_axis * y + z_axis` and stops, never
/// dividing by the `w` it just produced. Anyone converting a call site would
/// reach for it, and what comes out is a picture that is plausible and is not
/// of the transform that was asked for — the failure this renderer is built to
/// refuse.
///
/// So the divide is not optional here. This type offers
/// [`project_point2`](Self::project_point2), which divides, and
/// [`project_homogeneous`](Self::project_homogeneous), which hands back the
/// undivided triple for the one caller that wants it — the vertex path, where
/// the rasterizer does the divide and needs `w` to clip against first. There is
/// no method that maps a point without doing one or the other.
///
/// # Storage
///
/// glam's `Mat3` is column-major, so `z_axis.xy` is the translation and the
/// bottom row — the one that makes this projective — is spread across the three
/// columns as `(x_axis.z, y_axis.z, z_axis.z)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D(Mat3);

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<Affine2> for Transform2D {
    fn from(affine: Affine2) -> Self {
        Self::from_affine(affine)
    }
}

impl std::ops::Mul for Transform2D {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        Self(self.0 * rhs.0)
    }
}

impl Transform2D {
    pub const IDENTITY: Self = Self(Mat3::IDENTITY);

    /// Lift an affine into a homography, which is exact and free.
    pub fn from_affine(affine: Affine2) -> Self {
        let m = affine.matrix2;
        let t = affine.translation;
        Self(Mat3::from_cols(
            Vec3::new(m.x_axis.x, m.x_axis.y, 0.0),
            Vec3::new(m.y_axis.x, m.y_axis.y, 0.0),
            Vec3::new(t.x, t.y, 1.0),
        ))
    }

    /// The affine this is, or `None` if it carries perspective.
    pub fn to_affine(self) -> Option<Affine2> {
        self.is_affine().then(|| {
            let c = self.0.to_cols_array();
            Affine2::from_mat2_translation(
                Mat2::from_cols(Vec2::new(c[0], c[1]), Vec2::new(c[3], c[4])),
                Vec2::new(c[6], c[7]),
            )
        })
    }

    /// Whether the bottom row is `(0, 0, 1)`, so that `w` is one everywhere.
    ///
    /// Tested exactly rather than against a tolerance, and the distinction is
    /// worth stating because it looks like an oversight. The bottom row's first
    /// two entries have units of inverse length, so there is no scale-free
    /// threshold to compare them against: whether `0.001` is negligible depends
    /// entirely on how far across the plane the geometry reaches. What this
    /// answers is "was perspective asked for", which is a question about how the
    /// transform was built, and a transform built from an affine has exact
    /// zeros there. Composition preserves them exactly, since the product's
    /// bottom row is `(0, 0, 1)` times the other matrix.
    ///
    /// A caller wanting "close enough to affine over this region" is asking a
    /// different question, and it needs the region.
    pub fn is_affine(self) -> bool {
        let c = self.0.to_cols_array();
        c[2] == 0.0 && c[5] == 0.0 && c[8] == 1.0
    }

    /// Reduce the `dart:ui` four-by-four, which is column-major and sixteen long.
    ///
    /// Exact for the content this renderer draws rather than an approximation of
    /// it. Applying a four-by-four to `(x, y, 0, 1)` never reads the `z` column,
    /// and the `z` row only produces a depth nothing here has a use for — so
    /// what is left, rows and columns zero, one and three, is the whole of what
    /// the matrix means on the plane.
    pub fn from_column_major_4x4(m: &[f32; 16]) -> Self {
        Self(Mat3::from_cols(
            Vec3::new(m[0], m[1], m[3]),
            Vec3::new(m[4], m[5], m[7]),
            Vec3::new(m[12], m[13], m[15]),
        ))
    }

    /// The four-by-four this reduces from, with the `z` axis left alone.
    pub fn to_column_major_4x4(self) -> [f32; 16] {
        let c = self.0.to_cols_array();
        [
            c[0], c[1], 0.0, c[2], //
            c[3], c[4], 0.0, c[5], //
            0.0, 0.0, 1.0, 0.0, //
            c[6], c[7], 0.0, c[8],
        ]
    }

    /// The point mapped and divided.
    ///
    /// A point on the vanishing line has no image, and the divide reports that
    /// as an infinity or a NaN rather than a wrong finite answer — which is what
    /// every caller here already guards for with `is_finite`.
    pub fn project_point2(self, point: Vec2) -> Vec2 {
        let h = self.project_homogeneous(point);
        h.truncate() / h.z
    }

    /// The point mapped and *not* divided.
    ///
    /// What a vertex carries. The rasterizer divides, and it needs `w` first in
    /// order to clip against the plane where `w` reaches zero.
    pub fn project_homogeneous(self, point: Vec2) -> Vec3 {
        self.0 * Vec3::new(point.x, point.y, 1.0)
    }

    /// The inverse, or `None` if there is not one.
    ///
    /// An affine inverts as an affine, through `Affine2` rather than through the
    /// general path, which is cheaper and — the part worth writing down — makes
    /// the exactness this code's property instead of a dependency's.
    ///
    /// Everything downstream that asks [`is_affine`](Self::is_affine) is asking
    /// about an exact zero, and an inverse taken the general way arrives at that
    /// zero through the same adjugate arithmetic as every other entry. It does
    /// in fact land on it today: glam returns a bottom row of exactly
    /// `(0, -0, 1)` for a lifted affine, and negative zero compares equal, so
    /// routing affines through the general path would work. It would work
    /// because of how a dependency happens to arrange one division, which is not
    /// a thing this can check and not a thing a version bump would announce. The
    /// branch costs a comparison and removes the question.
    pub fn inverse(self) -> Option<Self> {
        if let Some(affine) = self.to_affine() {
            let determinant = affine.matrix2.determinant();
            if determinant.abs() <= 1e-9 || !determinant.is_finite() {
                return None;
            }
            let inverse = affine.inverse();
            return inverse.is_finite().then(|| Self::from_affine(inverse));
        }
        let determinant = self.0.determinant();
        if determinant.abs() <= 1e-9 || !determinant.is_finite() {
            return None;
        }
        let inverse = self.0.inverse();
        inverse.is_finite().then_some(Self(inverse))
    }

    /// The nine floats, in column order.
    pub fn to_cols_array(self) -> [f32; 9] {
        self.0.to_cols_array()
    }
}

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

/// Invert a paint's placement into the form a shader's `to_local` takes.
///
/// `local_to_clip` carries the paint's own space onto clip space — the canvas
/// transform, then wherever the paint sits, then whatever normalization its
/// kind wants, a radius or a rectangle's size. What comes back is the inverse,
/// as three columns of a three-by-three each padded to four floats, which is how
/// a `mat3x3` sits in a uniform block and what lets both backends copy the
/// packed material in without writing padding around anything.
///
/// # Why the whole matrix rather than a linear part and an anchor
///
/// It used to be four floats and a separately packed clip-space anchor, and the
/// shader subtracted the one before applying the other. That works only for an
/// affine, where the image of a difference is the difference of the images. A
/// map with perspective divides by a quantity that depends on the absolute
/// position, so an anchor subtracted before the mapping is subtracted in the
/// wrong space — correct only in the case that made it look correct. Inside the
/// matrix is the one place the translation belongs, and it rides there for free.
///
/// # A placement that has collapsed
///
/// A degenerate transform — a zero scale, or one axis collapsed — has no
/// inverse. That is a caller mistake rather than a renderer one, and the shape
/// it fills is collapsed to nothing anyway, so the paint it would have carried
/// is not observable: the geometry went through the same matrix the mapping is
/// the inverse of, which is the whole of why substituting anything here is safe.
///
/// The identity is what stands in, and it is chosen over anything else because
/// its bottom row is `(0, 0, 1)`. A fragment that reached it despite the above
/// would divide by one rather than by zero, and a NaN put into a fragment's
/// color survives the blend and spreads across whatever it touches.
pub fn invert_to_local(local_to_clip: Transform2D) -> [f32; 12] {
    to_local_columns(local_to_clip.inverse().unwrap_or(Transform2D::IDENTITY))
}

/// The same packing, for a mapping already stated in the direction the shader
/// reads it.
///
/// A layer composite and a picture's placement both build the clip-to-local
/// mapping directly, because they know the texture's size rather than a
/// placement to invert. Inverting a matrix only to invert it back would be
/// arithmetic with nothing to show for it.
pub fn to_local_columns(clip_to_local: Transform2D) -> [f32; 12] {
    let c = clip_to_local.to_cols_array();
    let padded = [
        c[0], c[1], c[2], 0.0, //
        c[3], c[4], c[5], 0.0, //
        c[6], c[7], c[8], 0.0,
    ];
    if padded.iter().all(|v| v.is_finite()) {
        padded
    } else {
        [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0,
        ]
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

    /// A homography is defined by where it sends four points, so checking it
    /// against a square's corners checks the whole map.
    #[test]
    fn a_homography_maps_a_square_to_the_quadrilateral_its_corners_describe() {
        // Bottom row (0.002, 0, 1): w grows with x, so the far side shrinks.
        let t = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        assert!(!t.is_affine());
        // At x = 100 the divisor is 1.2, at x = 0 it is 1.
        let near = t.project_point2(Vec2::new(0.0, 100.0));
        let far = t.project_point2(Vec2::new(100.0, 100.0));
        assert!(approx(near, Vec2::new(0.0, 100.0)));
        assert!(approx(far, Vec2::new(100.0 / 1.2, 100.0 / 1.2)));
        // The undivided form carries the divisor rather than applying it.
        let homogeneous = t.project_homogeneous(Vec2::new(100.0, 100.0));
        assert!((homogeneous.z - 1.2).abs() < 1e-5);
    }

    /// The property that lets every existing call site keep working.
    #[test]
    fn an_affine_lifts_and_lowers_without_moving_a_point() {
        let affine = Affine2::from_angle(0.3)
            * Affine2::from_scale(Vec2::new(2.0, 7.0))
            * Affine2::from_translation(Vec2::new(11.0, -3.0));
        let lifted = Transform2D::from(affine);
        assert!(lifted.is_affine());
        assert_eq!(lifted.to_affine(), Some(affine));
        for p in [Vec2::ZERO, Vec2::new(5.0, -2.0), Vec2::new(-100.0, 40.0)] {
            assert!(approx(lifted.project_point2(p), affine.transform_point2(p)));
            // And w stayed exactly one, which is what keeps the divide free.
            assert_eq!(lifted.project_homogeneous(p).z, 1.0);
        }
    }

    #[test]
    fn a_transform_round_trips_through_the_four_by_four_form() {
        let t = Transform2D::from_column_major_4x4(&[
            2.0, 0.5, 0.0, 0.003, //
            -1.0, 3.0, 0.0, 0.007, //
            0.0, 0.0, 1.0, 0.0, //
            9.0, -4.0, 0.0, 1.5,
        ]);
        assert_eq!(
            Transform2D::from_column_major_4x4(&t.to_column_major_4x4()),
            t
        );
    }

    #[test]
    fn composing_two_homographies_agrees_with_transforming_twice() {
        let a = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            5.0, 0.0, 0.0, 1.0,
        ]);
        let b = Transform2D::from(Affine2::from_angle(0.4) * Affine2::from_scale(Vec2::splat(2.0)));
        for p in [Vec2::new(3.0, 4.0), Vec2::new(-20.0, 60.0)] {
            assert!(approx(
                (a * b).project_point2(p),
                a.project_point2(b.project_point2(p))
            ));
        }
    }

    /// Composition must not manufacture perspective that nobody asked for, or
    /// every affine fast path downstream turns itself off after one `concat`.
    #[test]
    fn composing_two_affines_stays_exactly_affine() {
        let a = Transform2D::from(Affine2::from_angle(0.3));
        let b = Transform2D::from(Affine2::from_scale(Vec2::new(1e6, 1e-6)));
        assert!((a * b).is_affine());
        assert!((b * a).is_affine());
    }

    #[test]
    fn the_inverse_of_a_projective_map_undoes_it() {
        let t = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.001, //
            0.0, 0.0, 1.0, 0.0, //
            7.0, -2.0, 0.0, 1.0,
        ]);
        let inverse = t.inverse().expect("invertible");
        for p in [Vec2::new(10.0, 20.0), Vec2::new(-30.0, 5.0)] {
            assert!(approx(inverse.project_point2(t.project_point2(p)), p));
        }
    }

    /// The contract the shader's clamp rests on: forward `w` positive means the
    /// inverse's divisor is positive too, so a fragment in front of the
    /// vanishing line is never mistaken for one behind it.
    #[test]
    fn the_inverse_keeps_the_sign_of_the_divisor() {
        let t = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.001, //
            0.0, 0.0, 1.0, 0.0, //
            7.0, -2.0, 0.0, 1.0,
        ]);
        let inverse = t.inverse().expect("invertible");
        for p in [Vec2::new(10.0, 20.0), Vec2::new(-30.0, 5.0)] {
            let forward = t.project_homogeneous(p);
            assert!(forward.z > 0.0, "test point is in front");
            let back = inverse.project_homogeneous(forward.truncate() / forward.z);
            assert!(back.z > 0.0, "divisor flipped sign through the inverse");
        }
    }

    /// An affine's inverse has to come back exactly affine, not affine to
    /// within rounding, or `is_affine` answers no and the fast paths stop.
    #[test]
    fn the_inverse_of_an_affine_is_still_exactly_affine() {
        let affine = Affine2::from_angle(0.37)
            * Affine2::from_scale(Vec2::new(1e4, 3e-3))
            * Affine2::from_translation(Vec2::new(1234.0, -99.0));
        let inverse = Transform2D::from(affine).inverse().expect("invertible");
        assert!(inverse.is_affine());
        let c = inverse.to_cols_array();
        assert_eq!((c[2], c[5], c[8]), (0.0, 0.0, 1.0));
    }

    #[test]
    fn a_singular_matrix_has_no_inverse_to_report() {
        // Collapsed in one axis, which still has a well-defined image.
        assert!(Transform2D::from(Affine2::from_scale(Vec2::new(0.0, 1.0)))
            .inverse()
            .is_none());
        assert!(Transform2D::from(Affine2::from_scale(Vec2::ZERO))
            .inverse()
            .is_none());
        // Projective and singular: two columns the same.
        let degenerate = Transform2D::from_column_major_4x4(&[
            1.0, 2.0, 0.0, 3.0, //
            1.0, 2.0, 0.0, 3.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        assert!(degenerate.inverse().is_none());
    }
}
