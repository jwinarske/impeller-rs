//! Drawing through the public API.
//!
//! Everything here goes through the facade, naming no backend and reaching for
//! nothing below it. That is the check the API exists for: if a picture cannot
//! be drawn without dropping to `impeller-hal` or `impeller-renderer`, the
//! front door is incomplete regardless of how well the machinery behind it
//! works.

use impeller::{
    Atlas, BackendPreference, BlendMode, Canvas, Color, Context, Coverage, Extent2D, GlyphKey,
    GradientStop, Layer, Paint, PathBuilder, PixelFormat, PositionedGlyph, Rect, Vec2,
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

/// A four-by-four image with a different color in each quadrant.
fn quadrant_image() -> Vec<u8> {
    let mut pixels = vec![0u8; 4 * 4 * 4];
    for y in 0..4u32 {
        for x in 0..4u32 {
            let i = ((y * 4 + x) * 4) as usize;
            let color: [u8; 4] = match (x < 2, y < 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 0, 255],
            };
            pixels[i..i + 4].copy_from_slice(&color);
        }
    }
    pixels
}

#[test]
fn an_image_paint_draws_a_texture_through_the_api() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::image(0, Rect::from_size(128.0, 128.0)).with_anti_alias(false),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // The quadrants land where the image put them, with the top-left of the
    // image at the top-left of the destination.
    for (x, y, want, corner) in [
        (16u32, 16u32, [255u8, 0, 0, 255], "top-left"),
        (112, 16, [0, 255, 0, 255], "top-right"),
        (16, 112, [0, 0, 255, 255], "bottom-left"),
        (112, 112, [255, 255, 0, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {corner} quadrant sampled the wrong part of the image"
        );
    }
}

#[test]
fn an_image_travels_with_the_canvas_transform() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    // A half turn about the middle of the target exchanges opposite quadrants.
    // The image's mapping goes through the same transform as the geometry, so
    // a mapping computed in device pixels instead would leave it upright while
    // the shape turned.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.translate(64.0, 64.0);
    canvas.rotate(std::f32::consts::PI);
    canvas
        .draw_rect(
            Rect::new(-64.0, -64.0, 64.0, 64.0),
            &Paint::image(0, Rect::new(-64.0, -64.0, 64.0, 64.0)).with_anti_alias(false),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Bottom-right now holds what was top-left.
    assert_eq!(pixel(&pixels, 112, 112), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 16, 16), [255, 255, 0, 255]);
}

#[test]
fn drawing_an_image_paint_without_the_image_is_refused() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    // Sampling whatever happens to be bound would draw a plausible picture out
    // of a previous frame's texture, which is worse than failing.
    let result = ctx.draw(&mut surface, &canvas.finish());
    ctx.destroy_surface(surface);
    assert!(result.is_err(), "a paint sampling slot 0 drew without it");
}

/// Two overlapping opaque circles, drawn through `build`.
fn overlapping_circles(canvas: &mut Canvas, alpha: f32) {
    for center in [Vec2::new(52.0, 64.0), Vec2::new(76.0, 64.0)] {
        canvas
            .draw_circle(
                center,
                28.0,
                &Paint::fill(Color::linear(1.0, 0.0, 0.0, alpha)).with_anti_alias(false),
            )
            .expect("circle");
    }
}

#[test]
fn a_layer_applies_its_alpha_to_the_group_rather_than_to_each_shape() {
    let Some(mut ctx) = context() else { return };

    // Two overlapping half-transparent circles drawn directly show where they
    // cross, because the second blends over the first. The same pair inside a
    // half-transparent layer does not: the group is composited once, so the
    // overlap is no denser than the rest. That difference is the whole reason
    // save layers exist, and it is what a per-shape alpha cannot express.
    let mut direct = Canvas::new(SIZE);
    direct.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    overlapping_circles(&mut direct, 0.5);
    let direct = render(&mut ctx, direct);

    let mut grouped = Canvas::new(SIZE);
    grouped.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    grouped.save_layer(Layer::opacity(0.5));
    overlapping_circles(&mut grouped, 1.0);
    grouped.restore();
    let grouped = render(&mut ctx, grouped);

    // Away from the overlap the two agree: one shape at half alpha either way.
    let outside = (36u32, 64u32);
    assert_eq!(
        pixel(&direct, outside.0, outside.1),
        pixel(&grouped, outside.0, outside.1),
        "the two differ where only one circle covers"
    );

    // In the overlap they must not. Drawn directly the second circle blends
    // over the first and the red is denser; grouped, it is not.
    let overlap = (64u32, 64u32);
    let direct_red = pixel(&direct, overlap.0, overlap.1)[0];
    let grouped_red = pixel(&grouped, overlap.0, overlap.1)[0];
    assert!(
        direct_red > grouped_red + 8,
        "the overlap is {direct_red} drawn directly and {grouped_red} through a layer; \
         a layer that composited per shape would make these equal"
    );
    // And grouped, the overlap matches the rest of the group exactly.
    assert_eq!(
        pixel(&grouped, overlap.0, overlap.1),
        pixel(&grouped, outside.0, outside.1),
        "the group was not composited as one image"
    );
}

#[test]
fn a_layer_composites_with_its_own_blend_mode() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 1.0, 1.0));
    canvas.save_layer(Layer::default().with_blend(BlendMode::Plus));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    // Red added to blue rather than replacing it: the layer's blend mode
    // governs how the finished group meets what was underneath.
    assert_eq!(pixel(&pixels, 64, 64), [255, 0, 255, 255]);
}

#[test]
fn layers_nest() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // Two halvings compose to a quarter. An inner layer composited into the
    // outer one rather than straight onto the target is what makes this hold;
    // if both went to the target the result would be a half.
    canvas.save_layer(Layer::opacity(0.5));
    canvas.save_layer(Layer::opacity(0.5));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    let got = pixel(&pixels, 64, 64);
    for (channel, want) in got.iter().take(3).zip([64u8, 64, 64]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 2,
            "two half-opacity layers gave {got:?}, expected about a quarter"
        );
    }
}

#[test]
fn a_clip_in_force_when_a_layer_opens_confines_the_result() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // The layer draws unclipped into its own target, and the draw that
    // composites it is clipped instead. The visible result must be the same as
    // if the clip had applied to every shape in the layer.
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("clip");
    canvas.save_layer(Layer::default());
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 32, 64),
        [255, 255, 255, 255],
        "inside the clip"
    );
    assert_eq!(pixel(&pixels, 96, 64), [0, 0, 0, 255], "outside the clip");
}

#[test]
fn a_clip_inside_a_layer_does_not_escape_it() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer(Layer::default());
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("clipped");
    canvas.restore();
    // Outside the layer the clip is gone again, so this covers everything.
    canvas
        .draw_rect(
            Rect::new(64.0, 0.0, 128.0, 128.0),
            &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("unclipped");

    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 32, 64),
        [255, 0, 0, 255],
        "inside the layer clip"
    );
    assert_eq!(
        pixel(&pixels, 96, 64),
        [0, 255, 0, 255],
        "the layer's clip outlived the layer"
    );
}

#[test]
fn a_recording_without_layers_still_has_exactly_one_pass() {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(Rect::from_size(16.0, 16.0), &Paint::fill(Color::WHITE))
        .expect("rect");
    let recording = canvas.finish();
    assert_eq!(recording.layer_count(), 0);
    assert_eq!(recording.passes.len(), 1);
}

#[test]
fn a_layer_left_open_is_composited_rather_than_discarded() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer(Layer::default());
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    // No restore. An unbalanced save_layer is a caller mistake, and dropping
    // everything drawn since it would look like a rendering fault instead.
    let recording = canvas.finish();
    assert_eq!(recording.layer_count(), 1);

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    assert_eq!(pixel(&pixels, 64, 64), [255, 255, 255, 255]);
}

#[test]
fn the_backends_agree_on_a_layered_frame() {
    // Layers are the first thing here that renders more than one pass per
    // frame and samples a render target, and the two backends allocate,
    // transition and bind that target quite differently. Comparing the same
    // recording through both is what catches a difference that each on its own
    // would render plausibly.
    let (Ok(mut vulkan), Ok(mut gles)) = (
        Context::new(BackendPreference::Vulkan),
        Context::new(BackendPreference::Gles),
    ) else {
        eprintln!("skipping: both backends are needed");
        return;
    };

    let build = || {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.1, 0.1, 0.2, 1.0));
        canvas.save_layer(Layer::opacity(0.6));
        overlapping_circles(&mut canvas, 1.0);
        canvas.save_layer(Layer::opacity(0.5).with_blend(BlendMode::Plus));
        canvas
            .draw_rect(
                Rect::new(20.0, 20.0, 108.0, 60.0),
                &Paint::fill(Color::linear(0.0, 0.8, 0.4, 1.0)).with_anti_alias(false),
            )
            .expect("rect");
        canvas.restore();
        canvas.restore();
        canvas.finish()
    };

    let a = {
        let mut surface = vulkan
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        vulkan.draw(&mut surface, &build()).expect("draw");
        let pixels = vulkan.read(&mut surface).expect("read");
        vulkan.destroy_surface(surface);
        pixels
    };
    let b = {
        let mut surface = gles
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        gles.draw(&mut surface, &build()).expect("draw");
        let pixels = gles.read(&mut surface).expect("read");
        gles.destroy_surface(surface);
        pixels
    };

    // One unit: compositing a layer is arithmetic, converted to fixed point
    // once per pass, and the two need not round it identically.
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the backends differ by up to {worst} on a layered frame"
    );
    // And the frame is not simply the background: a comparison of two blank
    // images would agree perfectly and prove nothing.
    assert_ne!(
        pixel(&a, 64, 40),
        pixel(&a, 4, 124),
        "the layered frame drew nothing"
    );
}

/// An atlas holding two glyphs of known coverage.
///
/// Synthetic rather than rasterized from a font: shaping and font parsing are
/// out of scope, and coverage whose every texel is known exactly is a far
/// better thing to assert against than whatever a hinter produced.
fn two_glyph_atlas() -> (Atlas, GlyphKey, GlyphKey) {
    let mut atlas = Atlas::new(64);
    let solid = GlyphKey {
        font: 1,
        glyph: 1,
        size: 16,
    };
    let half = GlyphKey {
        font: 1,
        glyph: 2,
        size: 16,
    };
    atlas
        .insert(
            solid,
            &Coverage {
                width: 8,
                height: 8,
                texels: vec![255; 64],
            },
        )
        .expect("solid glyph");
    atlas
        .insert(
            half,
            &Coverage {
                width: 8,
                height: 8,
                texels: vec![128; 64],
            },
        )
        .expect("half glyph");
    (atlas, solid, half)
}

/// Upload an atlas as an image this context can sample.
fn upload_atlas(ctx: &mut Context, atlas: &Atlas) -> impeller::Image {
    let mut image = ctx
        .create_image(
            Extent2D::new(atlas.size(), atlas.size()),
            PixelFormat::Rgba8Unorm,
        )
        .expect("atlas image");
    // Coverage is one byte per texel and the format is four, so it is repeated
    // across the channels. The shader reads red; storing it four times costs
    // memory a single-channel format would save, and changes nothing else.
    let mut rgba = Vec::with_capacity(atlas.texels().len() * 4);
    for coverage in atlas.texels() {
        rgba.extend_from_slice(&[*coverage; 4]);
    }
    ctx.write_image(&mut image, &rgba).expect("upload");
    image
}

#[test]
fn a_glyph_run_draws_its_coverage_tinted_by_the_paint() {
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, half) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_glyphs(
            &[
                PositionedGlyph::new(solid, [16.0, 16.0], atlas.get(solid).unwrap()),
                PositionedGlyph::new(half, [48.0, 16.0], atlas.get(half).unwrap()),
            ],
            &atlas,
            0,
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)),
        )
        .expect("glyphs");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Full coverage gives the paint's color outright; half coverage gives it at
    // half alpha, premultiplied. That the two differ is the point: an atlas
    // read as color rather than as coverage would make both fully red.
    assert_eq!(pixel(&pixels, 20, 20), [255, 0, 0, 255], "the solid glyph");
    let faint = pixel(&pixels, 52, 20);
    assert!(
        (faint[0] as i32 - 128).abs() <= 2 && faint[1] == 0 && faint[2] == 0,
        "the half-coverage glyph came back {faint:?}"
    );
    // And nothing was drawn where no glyph was placed.
    assert_eq!(
        pixel(&pixels, 100, 100),
        [0, 0, 0, 255],
        "between the glyphs"
    );
}

#[test]
fn a_run_of_many_glyphs_is_one_draw() {
    // The reason vertices carry texture coordinates at all. A paint is per
    // draw, so coordinates carried there would mean a draw per glyph, and text
    // is the highest draw-count content there is.
    let (atlas, solid, _) = two_glyph_atlas();
    let rect = atlas.get(solid).unwrap();
    let glyphs: Vec<PositionedGlyph> = (0..24)
        .map(|i| PositionedGlyph::new(solid, [i as f32 * 5.0, 40.0], rect))
        .collect();

    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_glyphs(&glyphs, &atlas, 0, &Paint::fill(Color::WHITE))
        .expect("glyphs");
    let recording = canvas.finish();
    assert_eq!(
        recording.draw_count(),
        1,
        "a run of {} glyphs should be one draw",
        glyphs.len()
    );
}

#[test]
fn a_glyph_run_travels_with_the_transform_and_the_clip() {
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, _) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);
    let rect = atlas.get(solid).unwrap();

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // Clipped to the left half, then moved right by a translation. A run whose
    // quads were built in device pixels would ignore both.
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("clip");
    canvas.translate(24.0, 24.0);
    canvas
        .draw_glyphs(
            &[
                PositionedGlyph::new(solid, [0.0, 0.0], rect),
                PositionedGlyph::new(solid, [56.0, 0.0], rect),
            ],
            &atlas,
            0,
            &Paint::fill(Color::WHITE),
        )
        .expect("glyphs");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // The first glyph moved with the translation and survives the clip.
    assert_eq!(
        pixel(&pixels, 28, 28),
        [255, 255, 255, 255],
        "the first glyph"
    );
    // The second lands past the clip's edge and must not appear.
    assert_eq!(pixel(&pixels, 84, 28), [0, 0, 0, 255], "the second glyph");
}

#[test]
fn a_glyph_missing_from_the_atlas_is_refused() {
    let (atlas, solid, _) = two_glyph_atlas();
    let absent = GlyphKey {
        font: 9,
        glyph: 9,
        size: 9,
    };
    let mut canvas = Canvas::new(SIZE);
    // Sampling a glyph nobody added would read whatever texel sits at the
    // origin and draw a plausible smudge, which is worse than failing.
    let result = canvas.draw_glyphs(
        &[PositionedGlyph::new(
            absent,
            [0.0, 0.0],
            atlas.get(solid).unwrap(),
        )],
        &atlas,
        0,
        &Paint::fill(Color::WHITE),
    );
    assert!(result.is_err(), "a glyph outside the atlas was accepted");
}

#[test]
fn glyphs_take_a_solid_color_rather_than_a_gradient() {
    let (atlas, solid, _) = two_glyph_atlas();
    let mut canvas = Canvas::new(SIZE);
    // An atlas supplies coverage, not color, so a gradient has nowhere to go.
    // Picking its first stop would draw something plausible and wrong.
    let result = canvas.draw_glyphs(
        &[PositionedGlyph::new(
            solid,
            [0.0, 0.0],
            atlas.get(solid).unwrap(),
        )],
        &atlas,
        0,
        &Paint::linear_gradient(
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            vec![
                GradientStop::new(Color::WHITE, 0.0),
                GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 1.0),
            ],
        ),
    );
    assert!(result.is_err(), "a gradient paint was accepted for glyphs");
}

#[test]
fn the_backends_agree_on_a_glyph_run() {
    // A run is the first geometry here whose vertices carry texture
    // coordinates, and the two backends declare that attribute separately —
    // one in a pipeline's vertex input state, the other as a pointer into an
    // interleaved buffer. A stride or offset wrong on either produces a
    // plausible smear rather than nothing.
    let (Ok(mut vulkan), Ok(mut gles)) = (
        Context::new(BackendPreference::Vulkan),
        Context::new(BackendPreference::Gles),
    ) else {
        eprintln!("skipping: both backends are needed");
        return;
    };
    let (atlas, solid, half) = two_glyph_atlas();

    let build = || {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.1, 1.0));
        let glyphs: Vec<PositionedGlyph> = (0..6)
            .map(|i| {
                let key = if i % 2 == 0 { solid } else { half };
                PositionedGlyph::new(key, [8.0 + i as f32 * 18.0, 40.0], atlas.get(key).unwrap())
            })
            .collect();
        canvas
            .draw_glyphs(
                &glyphs,
                &atlas,
                0,
                &Paint::fill(Color::linear(1.0, 0.8, 0.2, 1.0)),
            )
            .expect("glyphs");
        canvas.finish()
    };

    let render_on = |ctx: &mut Context| {
        let image = upload_atlas(ctx, &atlas);
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &build(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        ctx.destroy_image(image);
        pixels
    };

    let a = render_on(&mut vulkan);
    let b = render_on(&mut gles);
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the backends differ by up to {worst} on a glyph run"
    );
    // And something was actually drawn, so this is not two blank frames
    // agreeing perfectly.
    assert_ne!(
        pixel(&a, 12, 44),
        pixel(&a, 120, 120),
        "the glyph run drew nothing"
    );
}

#[test]
fn a_glyph_run_is_correct_after_the_atlas_has_been_repacked() {
    let Some(mut ctx) = context() else { return };

    // Compaction moves every surviving glyph, so the coordinates a run was
    // going to read are wrong afterwards. The atlas reports itself dirty and
    // the run reads its rectangles at record time, and this is the end-to-end
    // check that those two together are enough: a caller that re-uploads when
    // told to gets the right picture, and one that does not would get a
    // different glyph's texels rather than a blank.
    let mut atlas = Atlas::new(32);
    let solid = GlyphKey {
        font: 1,
        glyph: 1,
        size: 16,
    };
    let marker = Coverage {
        width: 6,
        height: 6,
        texels: vec![255; 36],
    };
    atlas.insert(solid, &marker).expect("solid");

    // Fill the rest with glyphs nothing will ask for again.
    let mut filler = 2u16;
    while atlas
        .insert(
            GlyphKey {
                font: 1,
                glyph: filler,
                size: 16,
            },
            &Coverage {
                width: 6,
                height: 6,
                texels: vec![64; 36],
            },
        )
        .is_ok()
    {
        filler += 1;
        assert!(filler < 200, "the atlas never filled");
    }

    // A new frame that needs only the marker, then one more glyph — which is
    // what forces the repack.
    atlas.begin_frame();
    atlas.insert(solid, &marker).expect("refresh");
    atlas
        .insert(
            GlyphKey {
                font: 1,
                glyph: 999,
                size: 16,
            },
            &Coverage {
                width: 6,
                height: 6,
                texels: vec![128; 36],
            },
        )
        .expect("should make room");
    assert_eq!(atlas.compactions(), 1, "the atlas did not repack");
    assert!(atlas.is_dirty(), "a repacked atlas owes an upload");

    let image = upload_atlas(&mut ctx, &atlas);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_glyphs(
            &[PositionedGlyph::new(
                solid,
                [40.0, 40.0],
                atlas.get(solid).unwrap(),
            )],
            &atlas,
            0,
            &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)),
        )
        .expect("glyphs");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Full coverage after repacking, not the quarter coverage the filler
    // glyphs carried — which is what a stale rectangle would have sampled.
    assert_eq!(
        pixel(&pixels, 42, 42),
        [0, 255, 0, 255],
        "the repacked glyph sampled the wrong part of the atlas"
    );
}
