//! Drawing through the public API.
//!
//! Everything here goes through the facade, naming no backend and reaching for
//! nothing below it. That is the check the API exists for: if a picture cannot
//! be drawn without dropping to `impeller-hal` or `impeller-renderer`, the
//! front door is incomplete regardless of how well the machinery behind it
//! works.

use impeller::{
    BackendPreference, BlendMode, Canvas, Color, Context, Extent2D, GradientStop, Paint,
    PathBuilder, PixelFormat, Rect, Vec2,
};

const SIZE: Extent2D = Extent2D {
    width: 128,
    height: 128,
};

fn context() -> Option<Context> {
    match Context::new(BackendPreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable backend ({e})");
            None
        }
    }
}

/// Draw a recording and read the result back.
fn render(ctx: &mut Context, canvas: Canvas) -> Vec<u8> {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    pixels
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_context_reports_which_backend_it_chose() {
    let Some(ctx) = context() else { return };
    eprintln!(
        "backend: {} on {}",
        ctx.backend(),
        ctx.capabilities().device_name
    );
    // Capabilities must be reachable without knowing the backend, since that is
    // what everything above is supposed to branch on.
    assert!(ctx.capabilities().max_texture_size >= 2048);
}

#[test]
fn a_rectangle_lands_where_it_was_asked_to() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 96.0, 96.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    assert_eq!(pixel(&pixels, 64, 64), [255, 255, 255, 255], "inside");
    assert_eq!(pixel(&pixels, 8, 8), [0, 0, 0, 255], "outside");
    // User coordinates run down and right, so a rect at the top-left of user
    // space must appear at the top-left of the image.
    assert_eq!(pixel(&pixels, 64, 16), [0, 0, 0, 255], "above the rect");
}

#[test]
fn colours_are_specified_in_srgb_and_stored_linearly() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    // A mid grey as a design tool would give it.
    canvas.clear(Color::rgba8(128, 128, 128, 255));
    let pixels = render(&mut ctx, canvas);

    // The target holds linear values, so sRGB 128 lands near 55, not 128.
    // Storing 128 would mean the conversion never happened, and every blend
    // against this colour would then be wrong.
    let stored = pixel(&pixels, 4, 4)[0];
    assert!(
        (50..=60).contains(&stored),
        "sRGB 128 should store near 55 linear, got {stored}"
    );
}

#[test]
fn the_transform_stack_places_shapes_independently() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let paint = Paint::fill(Color::WHITE).with_anti_alias(false);

    canvas.save();
    canvas.translate(16.0, 16.0);
    canvas
        .draw_rect(Rect::from_size(16.0, 16.0), &paint)
        .unwrap();
    canvas.restore();

    // After restoring, this must be drawn in the original frame rather than
    // inheriting the translation above.
    canvas
        .draw_rect(Rect::from_size(16.0, 16.0), &paint)
        .unwrap();

    let pixels = render(&mut ctx, canvas);
    assert_eq!(pixel(&pixels, 24, 24), [255, 255, 255, 255], "translated");
    assert_eq!(pixel(&pixels, 8, 8), [255, 255, 255, 255], "restored");
    assert_eq!(pixel(&pixels, 60, 60), [0, 0, 0, 255], "neither");
}

#[test]
fn a_stroke_traces_an_outline_rather_than_filling_it() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 96.0, 96.0),
            &Paint::stroke(Color::WHITE, 8.0).with_anti_alias(false),
        )
        .expect("stroke");

    let pixels = render(&mut ctx, canvas);
    // On the edge, but not in the middle: the distinction between stroking and
    // filling is exactly the interior.
    assert_eq!(pixel(&pixels, 64, 32), [255, 255, 255, 255], "on the edge");
    assert_eq!(pixel(&pixels, 64, 64), [0, 0, 0, 255], "interior");
}

#[test]
fn translucent_paint_blends_with_what_is_underneath() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 1.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 0.5)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let got = pixel(&pixels, 64, 64);
    // Half red over opaque blue, computed rather than recorded.
    for (channel, want) in got.iter().zip([128u8, 0, 128, 255]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 2,
            "got {got:?}, expected about [128, 0, 128, 255]"
        );
    }
}

#[test]
fn antialiasing_is_requested_through_the_paint() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }

    let draw = |ctx: &mut Context, anti_alias: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                48.0,
                &Paint::fill(Color::WHITE).with_anti_alias(anti_alias),
            )
            .expect("circle");
        render(ctx, canvas)
    };

    let partial = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 0 && p[0] < 255)
            .count()
    };

    // A caller asks per paint; the canvas turns that into a pass that samples.
    assert_eq!(partial(&draw(&mut ctx, false)), 0, "aliased");
    assert!(partial(&draw(&mut ctx, true)) > 32, "antialiased");
}

#[test]
fn an_empty_frame_still_clears() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::rgba8(255, 255, 255, 255));
    let pixels = render(&mut ctx, canvas);
    assert_eq!(pixel(&pixels, 0, 0), [255, 255, 255, 255]);
}

#[test]
fn a_surface_from_one_context_is_refused_by_another() {
    let Some(mut first) = context() else { return };
    let Some(mut second) = context() else { return };

    let mut surface = first
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    // A surface holds memory the context that made it allocated. Using it with
    // another would pass a handle that device never issued, which is undefined
    // rather than merely wrong, so it has to be refused on the CPU.
    let canvas = Canvas::new(SIZE);
    if first.backend() == second.backend() {
        // Same backend means the enum arms match and the error cannot be
        // detected here; that check belongs to a handle generation the backends
        // do not carry yet.
        eprintln!("skipping: both contexts chose the same backend");
        first.destroy_surface(surface);
        return;
    }
    assert!(second.draw(&mut surface, &canvas.finish()).is_err());
    first.destroy_surface(surface);
}

#[test]
fn a_linear_gradient_runs_between_its_stops() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(128.0, 0.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let pixels = render(&mut ctx, canvas);
    let left = pixel(&pixels, 1, 64);
    let middle = pixel(&pixels, 64, 64);
    let right = pixel(&pixels, 126, 64);

    // Red at the start, blue at the end, and a genuine mix between: a gradient
    // that failed to evaluate would come back as one flat colour.
    assert!(left[0] > 240 && left[2] < 16, "left end: {left:?}");
    assert!(right[2] > 240 && right[0] < 16, "right end: {right:?}");
    assert!(
        middle[0] > 80 && middle[0] < 180 && middle[2] > 80 && middle[2] < 180,
        "midpoint should be a mix, got {middle:?}"
    );
}

#[test]
fn a_gradient_runs_along_the_axis_it_was_given() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    // Vertical this time. A gradient that ignored its endpoints, or that used
    // the fragment coordinate builtin instead of clip position, would run the
    // wrong way or not change at all.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(0.0, 128.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let pixels = render(&mut ctx, canvas);
    // User space runs downward, so the start colour belongs at the top.
    assert!(
        pixel(&pixels, 64, 1)[0] > 240,
        "top should be the first stop"
    );
    assert!(
        pixel(&pixels, 64, 126)[2] > 240,
        "bottom should be the last stop"
    );
    // And it must vary down the axis rather than across it.
    let a = pixel(&pixels, 4, 64);
    let b = pixel(&pixels, 124, 64);
    assert_eq!(a, b, "a vertical gradient should not vary horizontally");
}

#[test]
fn a_gradient_travels_with_the_canvas_transform() {
    let Some(mut ctx) = context() else { return };

    let draw = |ctx: &mut Context, rotate: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        if rotate {
            // Rotate about the centre by a quarter turn.
            canvas.translate(64.0, 64.0);
            canvas.rotate(std::f32::consts::FRAC_PI_2);
            canvas.translate(-64.0, -64.0);
        }
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::linear_gradient(
                    Vec2::new(0.0, 0.0),
                    Vec2::new(128.0, 0.0),
                    vec![
                        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                    ],
                )
                .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    let plain = draw(&mut ctx, false);
    let rotated = draw(&mut ctx, true);

    // Endpoints go through the same transform the geometry does, so rotating
    // the canvas turns the gradient with the shape rather than leaving it
    // pinned to the screen.
    assert!(
        pixel(&plain, 1, 64)[0] > 240,
        "unrotated starts red at left"
    );
    assert!(
        pixel(&rotated, 64, 1)[0] > 240,
        "rotated should start red at the top, got {:?}",
        pixel(&rotated, 64, 1)
    );
}

#[test]
fn a_gradient_with_more_than_two_stops_passes_through_each() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(128.0, 0.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 0.5),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let pixels = render(&mut ctx, canvas);
    // The middle stop has to actually be reached, not interpolated past. A
    // two-stop shortcut would show no green anywhere.
    let middle = pixel(&pixels, 64, 64);
    assert!(
        middle[1] > 200 && middle[0] < 60 && middle[2] < 60,
        "the middle stop should dominate at the midpoint, got {middle:?}"
    );
}

#[test]
fn a_radial_gradient_runs_outward_from_its_centre() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::radial_gradient(
                Vec2::new(64.0, 64.0),
                60.0,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("radial");

    let pixels = render(&mut ctx, canvas);
    let centre = pixel(&pixels, 64, 64);
    assert!(centre[0] > 240 && centre[2] < 16, "centre: {centre:?}");

    // Equidistant points must match. A radial gradient measured in clip space
    // without mapping back would be an ellipse, and these would differ.
    let right = pixel(&pixels, 64 + 40, 64);
    let below = pixel(&pixels, 64, 64 + 40);
    for channel in 0..4 {
        assert!(
            (right[channel] as i32 - below[channel] as i32).abs() <= 2,
            "the gradient is not circular: right {right:?}, below {below:?}"
        );
    }
    // And it must actually vary with distance.
    assert!(right[2] > centre[2] + 40, "no falloff toward the edge");
}

#[test]
fn a_radial_gradient_stays_circular_on_a_non_square_target() {
    let Some(mut ctx) = context() else { return };
    // The case the mapping exists for: clip space scales each axis by that
    // axis' size, so on a non-square target an unmapped radial gradient comes
    // out visibly stretched.
    let extent = Extent2D::new(192, 96);
    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(192.0, 96.0),
            &Paint::radial_gradient(
                Vec2::new(96.0, 48.0),
                40.0,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("radial");

    let mut surface = ctx
        .create_surface(extent, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);

    let at = |x: u32, y: u32| {
        let i = ((y * extent.width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    // Thirty pixels right and thirty pixels down from the centre are the same
    // distance away, so they must be the same colour.
    let right = at(96 + 30, 48);
    let below = at(96, 48 + 30);
    for channel in 0..4 {
        assert!(
            (right[channel] as i32 - below[channel] as i32).abs() <= 2,
            "stretched by the aspect ratio: right {right:?}, below {below:?}"
        );
    }
}

#[test]
fn a_sweep_gradient_runs_around_its_centre() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::sweep_gradient(
                Vec2::new(64.0, 64.0),
                0.0,
                std::f32::consts::TAU,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("sweep");

    let pixels = render(&mut ctx, canvas);
    // Angle rather than distance: two points at different radii along the same
    // ray must match, while two at the same radius on different rays must not.
    let near = pixel(&pixels, 64 + 12, 64);
    let far = pixel(&pixels, 64 + 48, 64);
    for channel in 0..4 {
        assert!(
            (near[channel] as i32 - far[channel] as i32).abs() <= 2,
            "a sweep should not vary with distance: near {near:?}, far {far:?}"
        );
    }

    // Clip Y runs up, so a point below the centre in the image is at a negative
    // angle and lands late in the sweep.
    let above = pixel(&pixels, 64, 64 - 40);
    assert_ne!(near, above, "a sweep should vary with angle");
}

#[test]
fn an_advanced_blend_mode_is_either_available_or_refused_through_the_api() {
    let Some(mut ctx) = context() else { return };

    // The whole contract in one place: a caller asks what the device can do,
    // and gets either the mode or an error. What must not happen is the third
    // outcome — a drawing that succeeds and is quietly source-over instead.
    let available = ctx.capabilities().advanced_blend;

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.6, 0.6, 0.6, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(0.5, 0.5, 0.5, 1.0))
                .with_blend(BlendMode::Multiply)
                .with_anti_alias(false),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    let drawn = ctx.draw(&mut surface, &canvas.finish());

    if available {
        drawn.expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        let got = pixel(&pixels, 64, 64);
        // Multiplying two opaque mid-grays gives their product, which is darker
        // than either. Asserting the direction rather than the value keeps this
        // about the API carrying the mode through, and leaves the arithmetic to
        // the conformance tests that check all eleven modes against the formula.
        let backdrop = pixel(
            &render(&mut ctx, {
                let mut plain = Canvas::new(SIZE);
                plain.clear(Color::linear(0.6, 0.6, 0.6, 1.0));
                plain
            }),
            64,
            64,
        );
        assert!(
            got[0] < backdrop[0],
            "multiply gave {got:?}, which is not darker than the backdrop {backdrop:?}"
        );
    } else {
        assert!(
            matches!(drawn, Err(impeller::Error::Unsupported(_))),
            "the device reports no advanced blending but the draw was not refused"
        );
    }
    ctx.destroy_surface(surface);
}

#[test]
fn a_clip_confines_drawing_to_a_rectangle() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // A clip nowhere near the middle of the target, so getting the origin or
    // an axis wrong lands somewhere visibly different rather than merely being
    // a pixel off.
    canvas
        .clip_rect(Rect::new(20.0, 10.0, 60.0, 40.0))
        .expect("clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(30, 20), "the middle of the clip was not drawn");
    assert!(
        lit(21, 11),
        "the top-left corner inside the clip was not drawn"
    );
    assert!(
        lit(59, 39),
        "the bottom-right corner inside the clip was not drawn"
    );
    for (x, y, where_) in [
        (19, 20, "left of the clip"),
        (60, 20, "right of the clip"),
        (30, 9, "above the clip"),
        (30, 40, "below the clip"),
        (100, 100, "far outside the clip"),
    ] {
        assert!(!lit(x, y), "{where_} was drawn anyway");
    }
}

#[test]
fn clips_intersect_rather_than_replacing_one_another() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // Two overlapping clips. Only their shared region may be drawn; a clip that
    // replaced its predecessor would leave the second rectangle whole.
    canvas.clip_rect(Rect::new(10.0, 10.0, 70.0, 50.0)).unwrap();
    canvas
        .clip_rect(Rect::new(40.0, 30.0, 100.0, 90.0))
        .unwrap();
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(50, 40), "the shared region was not drawn");
    assert!(
        !lit(20, 20),
        "a region only the first clip allowed was drawn"
    );
    assert!(
        !lit(80, 70),
        "a region only the second clip allowed was drawn"
    );
}

#[test]
fn restore_returns_the_clip_along_with_the_transform() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    canvas.save();
    canvas.clip_rect(Rect::new(0.0, 0.0, 20.0, 20.0)).unwrap();
    canvas.restore();

    // With the clip restored to nothing, this covers the whole target. A clip
    // that outlived its save would confine it to the top-left corner.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    assert!(
        pixel(&pixels, 100, 100)[0] > 128,
        "a clip outlived the save that scoped it"
    );
}

#[test]
fn a_clip_moves_with_the_transform_that_was_in_force_when_it_was_set() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // The clip is given in user space, so the translation applies to it. A clip
    // taken in device pixels instead would sit at the origin.
    canvas.translate(64.0, 64.0);
    canvas.clip_rect(Rect::new(0.0, 0.0, 40.0, 40.0)).unwrap();
    canvas
        .draw_rect(
            Rect::new(-128.0, -128.0, 128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(70, 70), "the clip did not travel with the transform");
    assert!(!lit(20, 20), "the clip stayed at the device origin");
    assert!(!lit(110, 110), "the clip extended past where it was placed");
}

#[test]
fn a_rotated_clip_is_the_rotated_shape_and_not_its_bounding_box() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A square clip turned through an eighth turn is a diamond in device
    // pixels. Its bounding box reaches the corners; the diamond does not, and
    // the difference is the whole reason this cannot be approximated -- a
    // bounding box would admit pixels the caller asked to remove.
    canvas.translate(64.0, 64.0);
    canvas.rotate(std::f32::consts::FRAC_PI_4);
    canvas
        .clip_rect(Rect::new(-30.0, -30.0, 30.0, 30.0))
        .expect("clip");
    canvas
        .draw_rect(
            Rect::new(-128.0, -128.0, 128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(64, 64), "the middle of the clip was not drawn");
    assert!(lit(64, 90), "a point inside the diamond was not drawn");
    // Inside the bounding box, which spans about 22 to 106 on both axes, but
    // outside the diamond. This is the pixel a bounding-box clip would keep.
    assert!(
        !lit(100, 100),
        "the clip reached a corner only its bounding box covers"
    );
    assert!(!lit(10, 64), "the clip reached outside its bounding box");
}

#[test]
fn a_clip_narrowed_to_nothing_draws_nothing() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.2, 0.4, 0.6, 1.0));
    // Two disjoint clips. This is a normal state for a subtree scrolled out of
    // view, not an error, so recording continues and simply produces nothing.
    canvas.clip_rect(Rect::new(0.0, 0.0, 20.0, 20.0)).unwrap();
    canvas.clip_rect(Rect::new(60.0, 60.0, 80.0, 80.0)).unwrap();
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)),
        )
        .expect("rect");

    let recording = canvas.finish();
    assert!(
        recording.is_empty(),
        "a draw that could reach no pixel was still recorded"
    );

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    // The clear still happens: an empty clip removes the drawing, not the frame.
    assert_eq!(pixel(&pixels, 64, 64), [51, 102, 153, 255]);
}

#[test]
fn a_path_clip_confines_drawing_to_the_path() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A triangle, which no rectangle approximates and no scissor expresses.
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(64.0, 16.0))
        .line_to(Vec2::new(112.0, 100.0))
        .line_to(Vec2::new(16.0, 100.0))
        .close();
    canvas.clip_path(&builder.build()).expect("clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(64, 80), "the middle of the triangle was not drawn");
    assert!(lit(64, 30), "near the apex was not drawn");
    // The corners of the triangle's bounding box, which the triangle misses.
    for (x, y, corner) in [(20, 20, "top-left"), (108, 20, "top-right")] {
        assert!(
            !lit(x, y),
            "the {corner} corner outside the triangle was drawn"
        );
    }
    assert!(!lit(64, 110), "below the triangle was drawn");
}

#[test]
fn a_path_clip_intersects_with_a_rectangular_one() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A scissor and a stencil clip in force at once. They are independent
    // mechanisms and both apply, which is what lets an axis-aligned clip keep
    // taking the cheap path even inside a path clip.
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("rect clip");
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(64.0, 16.0))
        .line_to(Vec2::new(112.0, 100.0))
        .line_to(Vec2::new(16.0, 100.0))
        .close();
    canvas.clip_path(&builder.build()).expect("path clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(50, 80), "the region both clips admit was not drawn");
    assert!(
        !lit(80, 80),
        "a region inside the triangle but outside the rectangle was drawn"
    );
    assert!(
        !lit(30, 30),
        "a region inside the rectangle but outside the triangle was drawn"
    );
}

#[test]
fn restore_undoes_a_path_clip() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A path clip lives in a buffer on the device rather than in the recorder,
    // so restoring it is a draw rather than an assignment. Getting that wrong
    // leaves every later shape confined to a clip that was supposed to have
    // been lifted.
    canvas.save();
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(64.0, 16.0))
        .line_to(Vec2::new(112.0, 100.0))
        .line_to(Vec2::new(16.0, 100.0))
        .close();
    canvas.clip_path(&builder.build()).expect("clip");
    canvas.restore();

    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    for (x, y) in [(4, 4), (124, 4), (4, 124), (124, 124), (64, 64)] {
        assert!(
            pixel(&pixels, x, y)[0] > 128,
            "({x}, {y}) was still confined by a clip that had been restored"
        );
    }
}

#[test]
fn nested_path_clips_intersect_and_unwind_one_level_at_a_time() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    let triangle = |apex_left: bool| {
        let mut builder = PathBuilder::new();
        if apex_left {
            builder
                .move_to(Vec2::new(16.0, 64.0))
                .line_to(Vec2::new(100.0, 16.0))
                .line_to(Vec2::new(100.0, 112.0))
                .close();
        } else {
            builder
                .move_to(Vec2::new(112.0, 64.0))
                .line_to(Vec2::new(28.0, 16.0))
                .line_to(Vec2::new(28.0, 112.0))
                .close();
        }
        builder.build()
    };

    canvas.save();
    canvas.clip_path(&triangle(true)).expect("outer");
    canvas.save();
    canvas.clip_path(&triangle(false)).expect("inner");
    // Inside both triangles.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let both = render(&mut ctx, canvas);

    // Now the same thing, unwinding one level before drawing: only the outer
    // triangle should confine it. A restore that stepped back too far or not
    // far enough gives a different picture from either.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save();
    canvas.clip_path(&triangle(true)).expect("outer");
    canvas.save();
    canvas.clip_path(&triangle(false)).expect("inner");
    canvas.restore();
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let outer_only = render(&mut ctx, canvas);

    let lit = |p: &[u8], x, y| pixel(p, x, y)[0] > 128;
    // A point inside the left-apex triangle but outside the right-apex one.
    assert!(!lit(&both, 22, 64), "the two clips did not intersect");
    assert!(
        lit(&outer_only, 22, 64),
        "restoring did not lift the inner clip"
    );
    // And the outer clip is still in force after the restore.
    assert!(
        !lit(&outer_only, 8, 64),
        "restoring lifted the outer clip as well"
    );
}
