//! Drawing through the public API.
//!
//! Everything here goes through the facade, naming no backend and reaching for
//! nothing below it. That is the check the API exists for: if a picture cannot
//! be drawn without dropping to `impeller-hal` or `impeller-renderer`, the
//! front door is incomplete regardless of how well the machinery behind it
//! works.

use impeller::{
    BackendPreference, Canvas, Color, Context, Extent2D, GradientStop, Paint, PixelFormat, Rect,
    Vec2,
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
