//! Drawing through the public API.
//!
//! Everything here goes through the facade, naming no backend and reaching for
//! nothing below it. That is the check the API exists for: if a picture cannot
//! be drawn without dropping to `impeller-hal` or `impeller-renderer`, the
//! front door is incomplete regardless of how well the machinery behind it
//! works.

use googletest::prelude::*;
use impeller::{
    Affine2, Atlas, BackendPreference, BlendMode, Canvas, Color, ColorFilter, Context, Coverage,
    Dash, Extent2D, GlyphKey, GradientStop, ImageFilter, Layer, LineCap, MaskBlurStyle, Paint,
    Path, PathBuilder, PixelFormat, PointMode, PositionedGlyph, Rect, Result, Sampling, SourceRect,
    Sprite, StrokeStyle, Style, TileMode, Vec2, VertexMode, Vertices, MAX_STOPS,
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

/// Draw an already-finished recording, for a test that inspects it first.
fn render_recording(ctx: &mut Context, recording: &impeller::Recording) -> Vec<u8> {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, recording).expect("draw");
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
    // A mid gray as a design tool would give it.
    canvas.clear(Color::rgba8(128, 128, 128, 255));
    let pixels = render(&mut ctx, canvas);

    // The target holds linear values, so sRGB 128 lands near 55, not 128.
    // Storing 128 would mean the conversion never happened, and every blend
    // against this color would then be wrong.
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

/// Every backend this build can reach, so a property that must hold on each is
/// stated once rather than per backend.
fn every_backend() -> Vec<Context> {
    [BackendPreference::Vulkan, BackendPreference::Gles]
        .into_iter()
        .filter_map(|preference| Context::new(preference).ok())
        .collect()
}

/// Fill a surface of the given format with one color and read a texel back.
fn flat_fill(ctx: &mut Context, format: PixelFormat, color: Color) -> [u8; 4] {
    let extent = Extent2D::new(16, 16);
    let mut canvas = Canvas::new(extent);
    canvas.clear(color);
    let mut surface = ctx.create_surface(extent, format).expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    [pixels[0], pixels[1], pixels[2], pixels[3]]
}

#[test]
fn an_srgb_image_decodes_when_it_is_sampled() {
    let Some(mut ctx) = context() else { return };
    // The other direction across the same boundary. A surface encodes on write;
    // an image decodes on sample. A caller uploading a picture has
    // sRGB-encoded bytes, because that is what every image file holds, and the
    // format is what says so -- read as linear they are too bright by exactly
    // the transfer function, which looks like a washed-out picture rather than
    // like a mistake.
    //
    // Neither choice fails, which is why this is worth pinning: both produce an
    // image, and only one produces the right one.
    let extent = Extent2D::new(32, 32);
    let encoded = 188u8; // mid gray, encoded
    let mut sampled = |format: PixelFormat| {
        let mut image = ctx
            .create_image(Extent2D::new(4, 4), format)
            .expect("image");
        ctx.write_image(&mut image, &[encoded; 4 * 4 * 4])
            .expect("upload");
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(32.0, 32.0),
                &Paint::image(0, Rect::from_size(32.0, 32.0)).with_anti_alias(false),
            )
            .expect("image paint");
        // Into a linear surface, so what comes back is the linear value the
        // shader sampled rather than a re-encoding of it.
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        ctx.destroy_image(image);
        pixels[0]
    };

    // Read as linear, the byte passes through unchanged -- which is right for
    // coverage or a lookup table, and wrong for a picture.
    assert!(
        sampled(PixelFormat::Rgba8Unorm).abs_diff(encoded) <= 1,
        "a linear image should sample the byte it was given"
    );
    // Read as sRGB, it decodes. The reference is the library's own conversion
    // rather than a constant, so this compares the device against the code
    // beside it.
    let want = (Color::srgb(
        encoded as f32 / 255.0,
        encoded as f32 / 255.0,
        encoded as f32 / 255.0,
        1.0,
    )
    .r * 255.0)
        .round() as u8;
    let got = sampled(PixelFormat::Rgba8UnormSrgb);
    assert!(
        got.abs_diff(want) <= 1,
        "an sRGB image sampled as {got} where decoding {encoded} gives {want}"
    );
    assert!(
        got < encoded,
        "decoding should darken an encoded byte, got {got} from {encoded}"
    );
}

#[test]
fn an_srgb_surface_returns_the_color_that_was_authored() {
    // The round trip is the reason both halves exist. A caller states a color
    // the way a designer picked it, in sRGB; the renderer converts to linear so
    // that blending and interpolation are done on light rather than on encoded
    // bytes; and an sRGB target encodes once on the way out. If all three agree
    // the byte that comes back is the byte that went in.
    //
    // Nothing exercised this. Every other test here uses a linear target,
    // because exact expected values are easier to state that way, so the format
    // that does the conversion had never been rendered into at all -- on either
    // backend, though both map it.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    for ctx in &mut contexts {
        let backend = ctx.backend();
        for authored in [0.2f32, 0.6, 0.85] {
            let got = flat_fill(
                ctx,
                PixelFormat::Rgba8UnormSrgb,
                Color::srgb(authored, authored, authored, 1.0),
            );
            let want = (authored * 255.0).round() as u8;
            // Exact, not close. A backend that skipped the conversion would
            // come back with the linear value, which for 0.6 is 81 rather than
            // 153 -- a difference far outside any rounding this could excuse.
            assert!(
                got[..3].iter().all(|c| c.abs_diff(want) <= 1),
                "{backend}: authored sRGB {authored} came back as {got:?}, wanted {want}"
            );
        }
    }
}

#[test]
fn an_srgb_surface_differs_from_a_linear_one_by_the_transfer_function() {
    // The stronger statement, and the one that says where the conversion
    // happens. The same drawing into the two formats must differ by exactly
    // the transfer -- which means the pipeline carried linear light the whole
    // way and only the final write encoded. A renderer that converted earlier,
    // or twice, would still round-trip and would fail this.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    for ctx in &mut contexts {
        let backend = ctx.backend();
        for value in [0.1f32, 0.35, 0.5, 0.9] {
            let color = Color::linear(value, value, value, 1.0);
            let linear = flat_fill(ctx, PixelFormat::Rgba8Unorm, color);
            let encoded = flat_fill(ctx, PixelFormat::Rgba8UnormSrgb, color);

            // The reference is the conversion the public API already offers,
            // so this compares the device against the library rather than
            // against a constant nobody can check.
            let want = (color.to_srgb()[0] * 255.0).round() as u8;
            assert!(
                linear[0].abs_diff((value * 255.0).round() as u8) <= 1,
                "{backend}: a linear target should hold the linear value, got {linear:?}"
            );
            assert!(
                encoded[0].abs_diff(want) <= 1,
                "{backend}: linear {value} encoded to {encoded:?}, wanted {want}"
            );
        }
    }
}

#[test]
fn the_backends_agree_on_an_srgb_surface() {
    // Two implementations converting differently is the failure this is for:
    // one applying the transfer in the shader and one leaving it to the
    // format would each look plausible alone and disagree by the transfer.
    let mut contexts = every_backend();
    if contexts.len() < 2 {
        eprintln!("skipping: both backends are needed");
        return;
    }
    let color = Color::srgb(0.42, 0.17, 0.73, 1.0);
    let first = flat_fill(&mut contexts[0], PixelFormat::Rgba8UnormSrgb, color);
    for ctx in &mut contexts[1..] {
        let got = flat_fill(ctx, PixelFormat::Rgba8UnormSrgb, color);
        assert_eq!(got, first, "the backends disagree on an sRGB surface");
    }
    // Not gray, so a channel swapped on the way through would show.
    assert!(
        first[0] != first[1] || first[1] != first[2],
        "the test color came back gray, so a channel swap would be invisible"
    );
}

/// Nest `count` identical stencil clips, draw, and return what came back.
///
/// The clip is a triangle rather than a rectangle so it cannot become a
/// scissor: the depth being counted lives in the stencil, and an axis-aligned
/// clip never reaches it.
fn deeply_clipped(ctx: &mut Context, count: usize, last: Option<&Path>) -> Result<Vec<u8>> {
    let extent = Extent2D::new(64, 64);
    let corner = |size: f32| {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(size, 0.0))
            .line_to(Vec2::new(0.0, size))
            .close();
        b.build()
    };
    let big = corner(64.0);

    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    for _ in 0..count {
        canvas.save();
        canvas.clip_path(&big).expect("clip");
    }
    if let Some(path) = last {
        canvas.save();
        canvas.clip_path(path).expect("clip");
    }
    canvas
        .draw_rect(
            Rect::from_size(64.0, 64.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("fill");
    for _ in 0..count + usize::from(last.is_some()) {
        canvas.restore();
    }

    let mut surface = ctx.create_surface(extent, PixelFormat::Rgba8Unorm)?;
    let outcome = ctx
        .draw(&mut surface, &canvas.finish())
        .and_then(|()| ctx.read(&mut surface));
    ctx.destroy_surface(surface);
    outcome
}

#[test]
fn a_clip_stack_deeper_than_the_stencil_is_refused_by_every_backend() {
    // The stencil counts nesting depth and every device is required to offer
    // eight bits of it, so 255 is the portable limit. Past it the count
    // saturates or wraps, and a later test for a depth that no longer fits
    // admits every pixel the clip was meant to exclude -- which draws content
    // the caller clipped away and looks like nothing at all.
    //
    // One backend refused and the other did not, so the same recording was an
    // error on Vulkan and a wrong picture on GLES. The check now lives beside
    // the depth it reads, because the limit belongs to the stencil format both
    // are required to offer rather than to either of them.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    for ctx in &mut contexts {
        let backend = ctx.backend();
        let error = deeply_clipped(ctx, 300, None)
            .err()
            .unwrap_or_else(|| panic!("{backend} accepted a clip stack of 300"));
        let text = error.to_string();
        assert!(
            text.contains("clip nesting depth") && text.contains("255"),
            "{backend} refused for the wrong reason: {text}"
        );
    }
}

#[test]
fn nesting_up_to_the_limit_still_works() {
    // The other half, and the one that would catch a limit set so low it broke
    // legitimate drawing: a refusal is only correct if what it refuses is past
    // what the stencil can actually hold.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    for ctx in &mut contexts {
        let backend = ctx.backend();
        let pixels = deeply_clipped(ctx, 255, None)
            .unwrap_or_else(|e| panic!("{backend} refused a legal clip stack of 255: {e}"));
        let at = |x: u32, y: u32| pixels[((y * 64 + x) * 4) as usize];
        assert_eq!(at(4, 4), 255, "{backend}: inside the clip should be drawn");
        assert_eq!(at(60, 60), 0, "{backend}: outside the clip should not be");
    }
}

#[test]
fn the_innermost_clip_still_applies_at_the_limit() {
    // Depth alone is not the property. What goes wrong past the limit is that
    // the innermost clip stops narrowing anything, so this nests to just under
    // the limit and then adds a much smaller clip: a point inside the outer
    // clips but outside the inner one must not be drawn. With the count
    // saturated that point was white, and nothing about it read as an error.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    let mut small = PathBuilder::new();
    small
        .move_to(Vec2::ZERO)
        .line_to(Vec2::new(16.0, 0.0))
        .line_to(Vec2::new(0.0, 16.0))
        .close();
    let small = small.build();

    for ctx in &mut contexts {
        let backend = ctx.backend();
        let pixels = deeply_clipped(ctx, 254, Some(&small))
            .unwrap_or_else(|e| panic!("{backend} refused a legal clip stack: {e}"));
        let at = |x: u32, y: u32| pixels[((y * 64 + x) * 4) as usize];
        assert_eq!(
            at(3, 3),
            255,
            "{backend}: inside every clip should be drawn"
        );
        assert_eq!(
            at(30, 10),
            0,
            "{backend}: the innermost clip stopped excluding anything"
        );
    }
}

#[test]
fn a_surface_from_one_context_is_refused_by_another() {
    // Two contexts of different kinds, asked for by name. Asking twice for
    // whichever is preferred returns the same kind both times, and the check
    // below cannot see a difference between two contexts of one backend --
    // that needs a handle generation the backends do not carry. So the test
    // used to notice it had nothing to compare and skip, which reads as a pass.
    let (Ok(mut first), Ok(mut second)) = (
        Context::new(BackendPreference::Vulkan),
        Context::new(BackendPreference::Gles),
    ) else {
        eprintln!("skipping: both backends are needed");
        return;
    };

    let mut surface = first
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    // A surface holds memory the context that made it allocated. Using it with
    // another would pass a handle that device never issued, which is undefined
    // rather than merely wrong, so it has to be refused on the CPU.
    let canvas = Canvas::new(SIZE);
    assert_ne!(first.backend(), second.backend());
    assert!(second.draw(&mut surface, &canvas.finish()).is_err());
    first.destroy_surface(surface);
}

#[test]
fn a_rounded_rectangle_cuts_its_corners_and_keeps_its_edges() {
    let Some(mut ctx) = context() else { return };
    // The shape an interface is mostly made of, and the one thing about it a
    // test can state simply: the corners come off and nothing else does.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let render = |ctx: &mut Context, radius: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(
                region,
                radius,
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rrect");
        render(ctx, canvas)
    };

    let square = render(&mut ctx, 0.0);
    let rounded = render(&mut ctx, 16.0);
    let lit = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] > 128;

    // Just inside each corner of the bounding box: filled when square, gone
    // when rounded. All four, since a corner built from the wrong tangent
    // tends to be wrong in one place rather than in all of them.
    for (x, y) in [(17, 33), (110, 33), (17, 94), (110, 94)] {
        assert!(
            lit(&square, x, y),
            "square corner ({x},{y}) should be filled"
        );
        assert!(
            !lit(&rounded, x, y),
            "rounded corner ({x},{y}) should be cut away"
        );
    }
    // The middle of each edge is untouched: rounding takes the corners only.
    for (x, y) in [(64, 33), (64, 94), (17, 64), (110, 64)] {
        assert!(
            lit(&rounded, x, y),
            "edge ({x},{y}) should survive rounding"
        );
    }
    assert!(lit(&rounded, 64, 64), "the middle should be filled");
}

#[test]
fn an_antialiased_rounded_rectangle_is_two_triangles() {
    // The point of evaluating the shape per fragment: the vertex cost stops
    // depending on how round it is. Tessellated, a corner is an arc flattened
    // to a tolerance; here it is arithmetic in the fragment stage, and the
    // geometry is the quad it runs over.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let vertices = |paint: &Paint| {
        let mut canvas = Canvas::new(SIZE);
        canvas.draw_rrect(region, 16.0, paint).expect("rrect");
        canvas
            .finish()
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum::<usize>()
    };

    let analytic = vertices(&Paint::fill(Color::WHITE));
    assert_eq!(analytic, 4, "an antialiased fill should be one quad");
    // The tessellated path is still there and still used, so this is a
    // comparison rather than a claim about what was replaced.
    let tessellated = vertices(&Paint::fill(Color::WHITE).with_anti_alias(false));
    assert!(
        tessellated > analytic * 4,
        "the tessellated path should cost many more vertices, got {tessellated}"
    );
}

#[test]
fn only_an_antialiased_solid_paint_takes_the_analytic_path() {
    // Everything else falls back to tessellation, and should: a gradient or an
    // image would need its own mapping and this one at once, which is the case
    // the push-constant budget was sized against, and an aliased paint is
    // asking for the hard edges the tessellated path gives.
    //
    // A stroke is not on that list. An outline is the same field narrowed to a
    // band, so it takes the same path -- which is what the outline test above
    // is about.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let vertices = |paint: &Paint, radius: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.draw_rrect(region, radius, paint).expect("rrect");
        canvas
            .finish()
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum::<usize>()
    };

    assert_eq!(
        vertices(&Paint::fill(Color::WHITE), 16.0),
        4,
        "the fast path"
    );
    // A radius of zero is not in the list below because it is a plain
    // rectangle either way, so a vertex count cannot tell which path drew it.
    // That it stays a plain rectangle is checked where the clamping is.
    for (name, paint, radius) in [
        (
            "aliased",
            Paint::fill(Color::WHITE).with_anti_alias(false),
            16.0,
        ),
        (
            "gradient",
            Paint::linear_gradient(
                Vec2::ZERO,
                Vec2::new(64.0, 0.0),
                vec![
                    GradientStop::new(Color::WHITE, 0.0),
                    GradientStop::new(Color::BLACK, 1.0),
                ],
            ),
            16.0,
        ),
    ] {
        assert!(
            vertices(&paint, radius) > 4,
            "{name} should have fallen back to tessellation"
        );
    }
}

#[test]
fn an_antialiased_rectangle_antialiases_without_multisampling() {
    let Some(mut ctx) = context() else { return };
    // A rectangle is four vertices whichever route it takes, so the distance
    // field buys nothing there. What it buys is the edge: the pass no longer
    // has to multisample for a shape that computes its own coverage, and a
    // frame made mostly of rectangles is the common case.
    //
    // Rotated and off the pixel grid, because an axis-aligned rectangle on
    // integer bounds has no edge to antialias and would show nothing either
    // way.
    let draw = |ctx: &mut Context, anti_alias: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.translate(64.0, 64.0);
        canvas.rotate(0.35);
        canvas.translate(-64.0, -64.0);
        canvas
            .draw_rect(
                Rect::new(30.5, 30.5, 97.5, 97.5),
                &Paint::fill(Color::WHITE).with_anti_alias(anti_alias),
            )
            .expect("rect");
        canvas.restore();
        let recording = canvas.finish();
        let samples = recording.root().descriptor.samples;
        let pixels = render_recording(ctx, &recording);
        let partial = pixels
            .chunks_exact(4)
            .filter(|texel| texel[0] > 8 && texel[0] < 247)
            .count();
        let coverage: u64 = pixels.chunks_exact(4).map(|texel| texel[0] as u64).sum();
        (samples, partial, coverage as f64)
    };

    let (samples, partial, coverage) = draw(&mut ctx, true);
    assert_eq!(
        samples, 1,
        "an antialiased rectangle should not multisample the pass"
    );
    assert!(
        partial > 100,
        "a rotated antialiased rectangle should have partly covered pixels, found {partial}"
    );

    // Still the right rectangle: a rotation preserves area, so the coverage is
    // the square's own however it is turned.
    let side = 97.5 - 30.5;
    let exact = side * side * 255.0;
    assert!(
        (coverage - exact).abs() / exact < 0.01,
        "the rotated rectangle covers {coverage} against an exact {exact}"
    );

    // And asking for hard edges still gives them.
    let (_, aliased_partial, _) = draw(&mut ctx, false);
    assert_eq!(
        aliased_partial, 0,
        "an aliased rectangle should have no partly covered pixels"
    );
}

/// The analytic shapes, drawn the same way by several tests below.
fn analytic_shapes(canvas: &mut Canvas) {
    canvas
        .draw_rrect(
            Rect::new(48.0, 38.0, 112.0, 70.0),
            12.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("rrect");
    canvas
        .draw_circle(
            Vec2::new(64.0, 80.0),
            9.0,
            &Paint::fill(Color::linear(1.0, 0.3, 0.2, 1.0)),
        )
        .expect("circle");
}

#[test]
fn an_analytic_shape_lands_the_same_in_a_bounded_layer() {
    let Some(mut ctx) = context() else { return };
    // A shape drawn from a distance field locates itself in clip space, and a
    // bounded layer's clip space is a smaller target at an offset inside the
    // frame. So the two have to agree about where that target is, in the same
    // way the geometry and the paint mappings already do -- and getting it
    // wrong would put the shape somewhere else in the layer while leaving it
    // the right shape, which no comparison against a tessellated version at
    // the frame's origin would notice.
    let extent = Extent2D::new(160, 120);
    let region = Rect::new(40.0, 30.0, 120.0, 90.0);
    let mut render_layered = |bounded: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        let layer = Layer::opacity(0.8);
        match bounded {
            true => canvas.save_layer_bounds(layer, region),
            false => canvas.save_layer(layer),
        };
        analytic_shapes(&mut canvas);
        canvas.restore();
        let recording = canvas.finish();
        let first = recording.passes[0].extent;
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (pixels, first)
    };

    let (full, full_extent) = render_layered(false);
    let (bounded, bounded_extent) = render_layered(true);
    assert_eq!(full_extent, extent, "an unbounded layer covers the frame");
    assert!(
        bounded_extent.width < extent.width,
        "the bounded layer should have gotten a smaller target"
    );
    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert_eq!(worst, 0, "the shape moved when the layer was bounded");
    assert!(full.iter().any(|&b| b > 64), "the shapes rendered nothing");
}

#[test]
fn a_clip_confines_an_analytic_shape_exactly() {
    let Some(mut ctx) = context() else { return };
    // A distance field is drawn onto a quad outset past the shape, so a clip
    // has to cut the quad rather than the shape it stands for. Checked as an
    // equality rather than a direction: inside the clip every pixel must match
    // the unclipped drawing, and outside it every pixel must be untouched.
    // Anything else -- a clip applied in the wrong space, or to the outset
    // rather than to what it covers -- moves pixels one way or the other.
    let extent = Extent2D::new(160, 120);
    let clip = Rect::new(30.0, 30.0, 100.0, 100.0);
    let mut render_clipped = |clipped: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        if clipped {
            canvas.save();
            canvas.clip_rect(clip).expect("clip");
        }
        analytic_shapes(&mut canvas);
        if clipped {
            canvas.restore();
        }
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let open = render_clipped(false);
    let confined = render_clipped(true);
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..extent.height {
        for x in 0..extent.width {
            let at = ((y * extent.width + x) * 4) as usize;
            let within = (x as f32) >= clip.left
                && (x as f32) < clip.right
                && (y as f32) >= clip.top
                && (y as f32) < clip.bottom;
            if within {
                assert_eq!(
                    &confined[at..at + 4],
                    &open[at..at + 4],
                    "({x}, {y}) is inside the clip and should be untouched by it"
                );
                if open[at] > 64 {
                    inside += 1;
                }
            } else {
                assert_eq!(
                    &confined[at..at + 4],
                    &[0, 0, 0, 255],
                    "({x}, {y}) is outside the clip and should not have been drawn"
                );
                if open[at] > 64 {
                    outside += 1;
                }
            }
        }
    }
    // The clip has to actually cut something, or both branches above are
    // satisfied by a shape that never reached its edge.
    assert!(
        inside > 0 && outside > 0,
        "the clip removed nothing: {inside} in, {outside} out"
    );
}

#[test]
fn an_outline_costs_the_distance_field_nothing() {
    let Some(mut ctx) = context() else { return };
    // A field already says how far every fragment is from the edge, so an
    // outline is the band where that is small -- the same two triangles and one
    // more subtraction. Tessellating one means building a second shape, offset
    // inward and outward with the corners resolved, which is where a stroked
    // path gets its vertices.
    let extent = Extent2D::new(200, 160);
    let mut stroked = |analytic: bool, kind: &str| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        let paint = Paint::stroke(Color::WHITE, 10.0).with_anti_alias(analytic);
        match kind {
            "rrect" => canvas
                .draw_rrect(Rect::new(30.0, 30.0, 170.0, 130.0), 24.0, &paint)
                .expect("rrect"),
            "circle" => canvas
                .draw_circle(Vec2::new(100.0, 80.0), 55.0, &paint)
                .expect("circle"),
            _ => canvas
                .draw_oval(Rect::new(20.0, 45.0, 180.0, 115.0), &paint)
                .expect("oval"),
        };
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (pixels, vertices)
    };

    for kind in ["rrect", "circle", "oval"] {
        let (analytic, analytic_vertices) = stroked(true, kind);
        let (tessellated, tessellated_vertices) = stroked(false, kind);
        assert_eq!(analytic_vertices, 4, "{kind}: an outline should be a quad");
        assert!(
            tessellated_vertices > 32,
            "{kind}: the tessellated outline should cost far more, got {tessellated_vertices}"
        );

        let area = |pixels: &[u8]| {
            pixels
                .chunks_exact(4)
                .map(|texel| texel[0] as u64)
                .sum::<u64>() as f64
        };
        let difference = (area(&analytic) - area(&tessellated)).abs() / area(&tessellated);
        assert!(
            difference < 0.01,
            "{kind}: the two outlines cover different amounts, {:.2}% apart",
            difference * 100.0
        );
        // An outline, not a fill: the middle of a shape far larger than the
        // width is untouched by either.
        let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * extent.width + x) * 4) as usize];
        assert_eq!(
            at(&analytic, 100, 80),
            0,
            "{kind}: the middle should be empty"
        );
        assert_eq!(
            at(&tessellated, 100, 80),
            0,
            "{kind}: the middle should be empty"
        );
    }

    // A stroked circle is an annulus, whose area is arithmetic rather than an
    // approximation of one -- so this is measured against the shape itself
    // rather than against the other way of drawing it.
    let (analytic, _) = stroked(true, "circle");
    let (tessellated, _) = stroked(false, "circle");
    let area = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>() as f64
    };
    let exact = std::f64::consts::PI * (60.0f64 * 60.0 - 50.0 * 50.0) * 255.0;
    let analytic_error = (area(&analytic) - exact).abs() / exact;
    let tessellated_error = (area(&tessellated) - exact).abs() / exact;
    assert!(
        analytic_error < 0.001,
        "the analytic ring is {:.3}% off the exact annulus",
        analytic_error * 100.0
    );
    assert!(
        analytic_error < tessellated_error,
        "the analytic ring should be nearer the exact annulus: {:.3}% against {:.3}%",
        analytic_error * 100.0,
        tessellated_error * 100.0
    );
}

#[test]
fn an_analytic_stroke_deforms_with_the_transform() {
    let Some(mut ctx) = context() else { return };
    // A stroke width is stated in the space the shape is, so a transform that
    // scales the axes differently should stretch the pen with everything else
    // -- a ring drawn twice as wide is twice as thick at its sides and no
    // thicker at its top. That is what a tessellated stroke does, because it
    // offsets the outline in path space and transforms the result, and the
    // field has to agree: converting the width to device space anywhere would
    // give a pen that stayed round while its shape stretched.
    let extent = Extent2D::new(220, 160);
    let mut ring = |analytic: bool, sx: f32, sy: f32| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.translate(110.0, 80.0);
        canvas.scale(sx, sy);
        canvas.translate(-110.0, -80.0);
        canvas
            .draw_circle(
                Vec2::new(110.0, 80.0),
                50.0,
                &Paint::stroke(Color::WHITE, 12.0).with_anti_alias(analytic),
            )
            .expect("ring");
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };
    // How many lit pixels a horizontal line through the center crosses, which
    // is both walls of the ring and so twice its thickness there.
    let across = |pixels: &[u8]| {
        (0..extent.width)
            .filter(|x| pixels[((80 * extent.width + x) * 4) as usize] > 128)
            .count()
    };
    let coverage = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>() as f64
    };

    let (round, round_tessellated) = (ring(true, 1.0, 1.0), ring(false, 1.0, 1.0));
    let (wide, wide_tessellated) = (ring(true, 2.0, 1.0), ring(false, 2.0, 1.0));

    // The pen stretched: twice as wide a ring is twice as thick at its sides.
    assert!(
        across(&wide) > across(&round) + 12,
        "the stroke did not stretch: {} against {}",
        across(&wide),
        across(&round)
    );
    // And matches what tessellating the same thing gives, which is the only
    // definition of right available here.
    for (name, analytic, tessellated) in [
        ("unscaled", &round, &round_tessellated),
        ("stretched", &wide, &wide_tessellated),
    ] {
        assert_eq!(
            across(analytic),
            across(tessellated),
            "{name}: the two strokes differ in thickness"
        );
        let apart = (coverage(analytic) - coverage(tessellated)).abs() / coverage(tessellated);
        assert!(
            apart < 0.01,
            "{name}: the two strokes cover different amounts, {:.2}% apart",
            apart * 100.0
        );
    }
}

#[test]
fn an_analytic_shape_does_not_erase_what_is_behind_it() {
    let Some(mut ctx) = context() else { return };
    // A shape evaluated per fragment is drawn on a quad larger than itself and
    // emits a fragment everywhere on it, including where coverage is nothing.
    // Under a mode that discards the destination where the source is
    // transparent, those fragments erase what is behind them -- in the ring
    // between the shape and its quad, and in a rounded corner that gap is most
    // of the corner. Tessellating covers only the shape and has no such gap, so
    // the two routes would disagree about a region neither is drawing into.
    //
    // The modes that cannot take this path are refused it rather than fixed,
    // because there is nothing to fix: the quad is what makes the edge smooth.
    let extent = Extent2D::new(128, 128);
    let shape = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut over_a_backdrop = |blend: BlendMode, anti_alias: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::linear(0.0, 0.4, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("backdrop");
        canvas
            .draw_rrect(
                shape,
                10.0,
                &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0))
                    .with_blend(blend)
                    .with_anti_alias(anti_alias),
            )
            .expect("shape");
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let pixels = render_recording(&mut ctx, &recording);
        (pixels, vertices)
    };

    let at = |pixels: &[u8], x: u32, y: u32| {
        let i = ((y * extent.width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    // Inside the quad and outside the shape: the corner of a rounded rectangle
    // whose arc has left this point behind, and the ring along a straight edge.
    let gaps = [(41, 41), (39, 64)];

    let (replaced, replaced_vertices) = over_a_backdrop(BlendMode::Src, true);
    let (tessellated, _) = over_a_backdrop(BlendMode::Src, false);
    // Refused the fast path, so it drew the outline it actually covers. Counted
    // against the mode that keeps it rather than against a bare number, since
    // the backdrop contributes vertices of its own to both.
    let (_, composited_vertices) = over_a_backdrop(BlendMode::SrcOver, true);
    assert!(
        replaced_vertices > composited_vertices,
        "a destination-discarding mode should have fallen back to tessellation:          {replaced_vertices} against {composited_vertices}"
    );
    for (x, y) in gaps {
        assert_eq!(
            at(&replaced, x, y),
            at(&tessellated, x, y),
            "({x}, {y}) differs between the two routes"
        );
        assert_ne!(
            at(&replaced, x, y),
            [0, 0, 0, 0],
            "({x}, {y}) was erased rather than left alone"
        );
    }

    // And the modes that can take it still do, still smoothly.
    let (composited, _) = over_a_backdrop(BlendMode::SrcOver, true);
    for (x, y) in gaps {
        assert_ne!(
            at(&composited, x, y),
            [0, 0, 0, 0],
            "({x}, {y}) was erased by a mode that composites"
        );
    }
}

#[test]
fn an_oval_is_an_ellipse_rather_than_a_stadium() {
    let Some(mut ctx) = context() else { return };
    // The shape a rounded rectangle cannot become. Past half its shorter side a
    // rounded rectangle stops changing and is a stadium: straight along the
    // middle, semicircular at the ends. An ellipse curves the whole way, and
    // the distinguishing measurement is halfway along the major axis, where it
    // has narrowed and a stadium has not.
    let extent = Extent2D::new(200, 140);
    // 160 by 72, so the semi-axes are 80 and 36 about (100, 70).
    let bounds = Rect::new(20.0, 34.0, 180.0, 106.0);
    let mut oval = |anti_alias: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas
            .draw_oval(
                bounds,
                &Paint::fill(Color::WHITE).with_anti_alias(anti_alias),
            )
            .expect("oval");
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (pixels, vertices)
    };

    let (analytic, analytic_vertices) = oval(true);
    let (tessellated, tessellated_vertices) = oval(false);
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * extent.width + x) * 4) as usize];

    assert_eq!(analytic_vertices, 4, "an antialiased oval should be a quad");
    assert!(
        tessellated_vertices > 16,
        "the tessellated oval should cost many more vertices, got {tessellated_vertices}"
    );

    for (name, pixels) in [("analytic", &analytic), ("tessellated", &tessellated)] {
        // Halfway along the major axis the curve has narrowed to 31 of its 36,
        // so a point 33 out is outside it and one 28 out is inside. A stadium
        // would have both inside, and so would a rounded rectangle asked for a
        // radius larger than it can take.
        assert_eq!(
            at(pixels, 140, 103),
            0,
            "{name}: this shape is a stadium, not an ellipse"
        );
        assert_eq!(at(pixels, 140, 98), 255, "{name}: inside the curve");
        // And the extremes of both axes are reached.
        assert_eq!(at(pixels, 100, 70), 255, "{name}: center");
        assert_eq!(at(pixels, 22, 70), 255, "{name}: the end of the major axis");
        assert_eq!(
            at(pixels, 100, 36),
            255,
            "{name}: the end of the minor axis"
        );
        assert_eq!(at(pixels, 25, 38), 0, "{name}: the corner is outside");
    }

    // Against the area of an actual ellipse, which neither is measured against
    // anywhere else. The distance field is nearer, as it was for the circle and
    // for the same reason: four cubics approximate a curve, and this is one.
    let exact = std::f64::consts::PI * 80.0 * 36.0 * 255.0;
    let area = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>() as f64
    };
    let analytic_error = (area(&analytic) - exact).abs() / exact;
    let tessellated_error = (area(&tessellated) - exact).abs() / exact;
    assert!(
        analytic_error < 0.001,
        "the analytic oval is {:.3}% off the exact area",
        analytic_error * 100.0
    );
    assert!(
        analytic_error < tessellated_error,
        "the analytic oval should be nearer the exact area: {:.3}% against {:.3}%",
        analytic_error * 100.0,
        tessellated_error * 100.0
    );
}

#[test]
fn an_antialiased_circle_uses_the_same_distance_field() {
    let Some(mut ctx) = context() else { return };
    // A circle is a rounded rectangle: a square whose corner radius is half its
    // side has no straight edge left, and the field reduces exactly to the
    // distance from the center less the radius. So it costs two triangles and
    // needs no shader of its own.
    let extent = Extent2D::new(160, 120);
    let radius = 40.0f32;
    let mut draw = |analytic: bool| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        if analytic {
            canvas
                .draw_circle(Vec2::new(80.0, 60.0), radius, &Paint::fill(Color::WHITE))
                .expect("circle");
        } else {
            canvas
                .draw_circle(
                    Vec2::new(80.0, 60.0),
                    radius,
                    &Paint::fill(Color::WHITE).with_anti_alias(false),
                )
                .expect("circle");
        }
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        let coverage: u64 = pixels.chunks_exact(4).map(|texel| texel[0] as u64).sum();
        (vertices, coverage as f64)
    };

    let (analytic_vertices, analytic_coverage) = draw(true);
    let (tessellated_vertices, tessellated_coverage) = draw(false);

    assert_eq!(
        analytic_vertices, 4,
        "an antialiased circle should be a quad"
    );
    assert!(
        tessellated_vertices > 16,
        "the tessellated circle should cost many more vertices, got {tessellated_vertices}"
    );

    // Against the area of an actual circle, which is a thing both are trying to
    // be and neither is measured against anywhere else. The analytic one is
    // nearer, and by a wide margin: the other is a polygon approximation with
    // its coverage quantized to four samples, where this is the curve itself
    // with coverage taken from a distance.
    let exact = std::f64::consts::PI * (radius as f64) * (radius as f64) * 255.0;
    let analytic_error = (analytic_coverage - exact).abs() / exact;
    let tessellated_error = (tessellated_coverage - exact).abs() / exact;
    assert!(
        analytic_error < 0.002,
        "the analytic circle is {:.3}% off the exact area",
        analytic_error * 100.0
    );
    assert!(
        analytic_error < tessellated_error,
        "the analytic circle should be nearer the exact area than the \
         tessellated one: {:.3}% against {:.3}%",
        analytic_error * 100.0,
        tessellated_error * 100.0
    );
}

#[test]
fn the_analytic_shape_matches_the_tessellated_one() {
    let Some(mut ctx) = context() else { return };
    // Two quite different ways of deciding which pixels the shape covers, held
    // against each other. Per pixel they differ at the edge -- four samples can
    // only say nothing, a quarter, a half, three quarters or all, where a
    // distance says anything in between -- so the comparison is of total
    // coverage, which is the same question both are answering.
    //
    // Under anisotropic scale as well, since that is where measuring the
    // distance in the wrong space would round the corners by different amounts
    // on each axis and still look plausible.
    let extent = Extent2D::new(160, 120);
    let region = Rect::new(40.0, 30.0, 120.0, 90.0);
    let mut coverage = |analytic: bool, sx: f32, sy: f32| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.translate(80.0, 60.0);
        canvas.scale(sx, sy);
        canvas.translate(-80.0, -60.0);
        if analytic {
            canvas
                .draw_rrect(region, 20.0, &Paint::fill(Color::WHITE))
                .expect("rrect");
        } else {
            canvas
                .draw_path(&region.to_rounded_path(20.0), &Paint::fill(Color::WHITE))
                .expect("path");
        }
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>()
    };

    for (sx, sy) in [(1.0, 1.0), (1.4, 0.8), (0.6, 1.3)] {
        let analytic = coverage(true, sx, sy) as f64;
        let tessellated = coverage(false, sx, sy) as f64;
        let error = (analytic - tessellated).abs() / tessellated;
        assert!(
            error < 0.01,
            "at scale ({sx}, {sy}) the two paths cover different areas: \
             {analytic} against {tessellated}, {:.2}% apart",
            error * 100.0
        );
    }
}

#[test]
fn the_analytic_shape_antialiases_without_multisampling() {
    let Some(mut ctx) = context() else { return };
    // What the distance field buys beyond the vertex count. A tessellated
    // shape at one sample has hard edges; this one does not, because coverage
    // is a number the fragment stage computes rather than a count of samples.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rrect(
            Rect::new(16.0, 32.0, 112.0, 96.0),
            16.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("rrect");
    let recording = canvas.finish();
    // One sample: nothing here asked for multisampling, and the analytic path
    // does not turn it on.
    assert_eq!(
        recording.root().descriptor.samples,
        1,
        "the analytic path should not need a multisampled pass"
    );

    let pixels = render_recording(&mut ctx, &recording);
    let partial = pixels
        .chunks_exact(4)
        .filter(|texel| texel[0] > 8 && texel[0] < 247)
        .count();
    assert!(
        partial > 40,
        "an analytic edge should have partly covered pixels, found {partial}"
    );
}

#[test]
fn a_radius_larger_than_the_rectangle_is_clamped() {
    let Some(mut ctx) = context() else { return };
    // Past half the shorter side the corner arcs would overlap and the outline
    // would cross itself, which fills as something nobody asked for. Clamped,
    // the shape becomes a stadium and then stops changing -- so an enormous
    // radius and a merely sufficient one give the same picture.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let render = |ctx: &mut Context, radius: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(
                region,
                radius,
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rrect");
        render(ctx, canvas)
    };

    // Half the height is 32, so both of these clamp to the same shape.
    let exact = render(&mut ctx, 32.0);
    let enormous = render(&mut ctx, 10_000.0);
    assert_eq!(exact, enormous, "a huge radius should clamp to the stadium");
    // Infinity among them: a caller who writes it means the roundest shape
    // available, so answering with a square would be the opposite of the ask.
    assert_eq!(
        exact,
        render(&mut ctx, f32::INFINITY),
        "an infinite radius should clamp like any other large one"
    );

    // And it is a stadium rather than an outline that crossed itself: the
    // middle is filled, the corners are gone, and the mid-height ends are the
    // extreme points of the curve.
    let lit = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] > 128;
    assert!(lit(&exact, 64, 64), "the middle should be filled");
    assert!(!lit(&exact, 17, 33), "the corner should be gone");
    assert!(lit(&exact, 17, 64), "the left end should reach its extreme");
}

#[test]
fn a_radius_of_zero_is_the_plain_rectangle() {
    // So a caller can pass a radius that happens to be zero -- an interface
    // animating one, or reading it from a style -- without special-casing it.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rrect(region, 0.0, &Paint::fill(Color::WHITE))
        .expect("rrect");
    let rounded = canvas.finish();

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(region, &Paint::fill(Color::WHITE))
        .expect("rect");
    let square = canvas.finish();

    assert_eq!(
        rounded.draw_count(),
        square.draw_count(),
        "a zero radius should record the same drawing as a plain rectangle"
    );
    // Negative too, which is what a caller subtracting a border can produce --
    // and a radius that is not a number at all, which is what arithmetic on one
    // produces when it goes wrong. Both give the plain rectangle rather than a
    // shape derived from a value nobody meant.
    for radius in [-4.0, f32::NAN] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(region, radius, &Paint::fill(Color::WHITE))
            .expect("rrect");
        let recorded = canvas.finish();
        assert_eq!(
            recorded.draw_count(),
            square.draw_count(),
            "a radius of {radius} should record the same drawing as a plain rectangle"
        );
    }
}

#[test]
fn an_empty_rounded_rectangle_draws_nothing() {
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_rrect(
            Rect::new(50.0, 50.0, 50.0, 20.0),
            8.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("rrect");
    assert!(
        canvas.finish().is_empty(),
        "a rectangle with no area should record no drawing"
    );
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
    // that failed to evaluate would come back as one flat color.
    assert!(left[0] > 240 && left[2] < 16, "left end: {left:?}");
    assert!(right[2] > 240 && right[0] < 16, "right end: {right:?}");
    assert!(
        middle[0] > 80 && middle[0] < 180 && middle[2] > 80 && middle[2] < 180,
        "midpoint should be a mix, got {middle:?}"
    );
}

#[test]
fn a_linear_gradient_runs_the_same_way_on_a_target_that_is_not_square() {
    let Some(mut ctx) = context() else { return };
    // Twice as wide as it is tall, which is what makes this fail: the shader
    // locates itself from clip position, and clip space is normalized to each
    // axis independently. Projecting onto the axis there scales x and y by
    // different amounts, so a diagonal gradient runs at the wrong angle — the
    // steeper the target, the further off. It has to be projected in the space
    // the caller stated the axis in.
    let extent = Extent2D {
        width: 128,
        height: 64,
    };
    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 64.0),
            &Paint::linear_gradient(
                Vec2::ZERO,
                // Diagonal, and equal in both axes so that the two sample points
                // below are the same distance along it.
                Vec2::new(64.0, 64.0),
                vec![
                    GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let mut surface = ctx
        .create_surface(extent, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    let at = |x: u32, y: u32| pixels[((y * extent.width + x) * 4) as usize];

    // (32, 0), (0, 32) and (16, 16) all project to the same point on a (1, 1)
    // axis, so they are the same color or the gradient is not running along
    // that axis. Before the fix these read 27, 104 and 66.
    let (a, b, c) = (at(32, 0), at(0, 32), at(16, 16));
    let spread = a.abs_diff(b).max(b.abs_diff(c)).max(a.abs_diff(c));
    assert!(
        spread <= 2,
        "equidistant points along the axis differ: {a}, {b}, {c}"
    );
    // And it is a gradient rather than a flat fill: twice as far along is
    // visibly brighter, and the near corner is dark.
    assert!(at(32, 32) > a + 40, "{} should exceed {a}", at(32, 32));
    assert!(at(1, 1) < 16, "the origin corner should be dark");
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
    // User space runs downward, so the start color belongs at the top.
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
            // Rotate about the center by a quarter turn.
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
    let center = pixel(&pixels, 64, 64);
    assert!(center[0] > 240 && center[2] < 16, "center: {center:?}");

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
    assert!(right[2] > center[2] + 40, "no falloff toward the edge");
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
    // Thirty pixels right and thirty pixels down from the center are the same
    // distance away, so they must be the same color.
    let right = at(96 + 30, 48);
    let below = at(96, 48 + 30);
    for channel in 0..4 {
        assert!(
            (right[channel] as i32 - below[channel] as i32).abs() <= 2,
            "stretched by the aspect ratio: right {right:?}, below {below:?}"
        );
    }
}

/// Render a 128-wide band filled by a gradient spanning only its first quarter.
///
/// The ramp is red at x=0 and blue at x=32, so everything from 32 on is outside
/// it -- three quarters of the picture rather than a strip at the edge, which
/// is what makes the three modes tell themselves apart at a glance and in
/// arithmetic.
fn tiled_ramp(ctx: &mut Context, tile: TileMode) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    // A white ground, so decal's "nothing" is visible as something rather than
    // as the clear color it would be indistinguishable from.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("ground");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(32.0, 0.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_tile_mode(tile)
            .with_blend(BlendMode::SrcOver)
            .with_anti_alias(false),
        )
        .expect("gradient");
    render(ctx, canvas)
}

#[test]
fn no_number_a_caller_can_write_takes_the_process_down() {
    let Some(mut ctx) = context() else { return };
    // A wider sweep than the NaN one beside this, and a weaker claim. What a
    // renderer draws for an infinite rectangle or a stroke a million units wide
    // is a judgement call; that it must not abort the program holding it is
    // not. Two of these were found aborting, inside a dependency's assertion,
    // which is a failure a caller can neither catch nor prevent.
    //
    // Every value here is one a caller reaches by ordinary means: a subtraction
    // that went negative, a division by an extent that turned out to be zero, a
    // coordinate scaled once too often.
    let poisons = [
        ("nan", f32::NAN),
        ("inf", f32::INFINITY),
        ("-inf", f32::NEG_INFINITY),
        ("huge", 1e30),
        ("-huge", -1e30),
        ("tiny", 1e-30),
        ("zero", 0.0),
        ("negative", -8.0),
    ];

    for (name, v) in poisons {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let white = Paint::fill(Color::WHITE);
        let stroked = Paint::stroke(Color::WHITE, v);

        // Shapes, sized and positioned by the value.
        let _ = canvas.draw_rect(Rect::new(v, v, v + 32.0, v + 32.0), &white);
        let _ = canvas.draw_rrect(Rect::from_size(64.0, 64.0), v, &white);
        let _ = canvas.draw_circle(Vec2::new(64.0, 64.0), v, &white);
        let _ = canvas.draw_oval(Rect::new(0.0, 0.0, v, v), &white);
        let _ = canvas.draw_line(Vec2::ZERO, Vec2::new(v, v), &stroked);
        let _ = canvas.draw_rect(Rect::from_size(32.0, 32.0), &stroked);

        // Curves, where the value is a control point rather than an endpoint.
        let mut builder = PathBuilder::new();
        builder.move_to(Vec2::new(8.0, 8.0));
        builder.quad_to(Vec2::new(v, 64.0), Vec2::new(96.0, 96.0));
        builder.cubic_to(Vec2::new(v, v), Vec2::new(16.0, v), Vec2::new(8.0, 8.0));
        builder.close();
        let _ = canvas.draw_path(&builder.build(), &white);

        // Paint parameters rather than geometry.
        let _ = canvas.draw_rect(
            Rect::from_size(64.0, 64.0),
            &Paint::radial_gradient(
                Vec2::new(64.0, 64.0),
                v,
                vec![
                    GradientStop::new(Color::WHITE, v),
                    GradientStop::new(Color::BLACK, 1.0),
                ],
            ),
        );
        let _ = canvas.draw_rect(
            Rect::from_size(64.0, 64.0),
            &Paint::sweep_gradient(
                Vec2::new(64.0, 64.0),
                v,
                v,
                vec![
                    GradientStop::new(Color::WHITE, 0.0),
                    GradientStop::new(Color::BLACK, 1.0),
                ],
            ),
        );

        // The state stack: a clip nothing can satisfy, a transform that
        // collapses, a layer scaled by a value that is not a fraction.
        canvas.save();
        let _ = canvas.clip_rect(Rect::new(v, v, v, v));
        canvas.translate(v, v);
        canvas.scale(v, v);
        canvas.rotate(v);
        let _ = canvas.draw_rect(Rect::from_size(16.0, 16.0), &white);
        canvas.restore();

        canvas.save_layer(Layer::opacity(v).with_blur(v));
        let _ = canvas.draw_rect(Rect::from_size(48.0, 48.0), &white);
        canvas.restore();

        // Rendered rather than only recorded: half of what could go wrong
        // happens in the backend, and a recording nobody submits proves
        // nothing about it.
        let pixels = render(&mut ctx, canvas);
        assert_eq!(
            pixels.len(),
            (SIZE.width * SIZE.height * 4) as usize,
            "{name} did not produce a full frame"
        );
    }
}

#[test]
fn a_glyph_placed_by_arithmetic_that_went_wrong_still_renders_a_frame() {
    let Some(mut ctx) = context() else { return };
    // Text layout is where a caller most easily produces one of these without
    // noticing: a width divided by a zero advance, an origin accumulated across
    // a run that began with a NaN. Separate from the sweep beside this because
    // a glyph run samples the atlas, and a draw whose texture nobody supplied
    // is refused before it can reach anything worth testing.
    let (atlas, solid, _) = two_glyph_atlas();
    let rect = atlas.get(solid).unwrap();

    for (name, v) in [
        ("nan", f32::NAN),
        ("inf", f32::INFINITY),
        ("-inf", f32::NEG_INFINITY),
        ("huge", 1e30),
    ] {
        let image = upload_atlas(&mut ctx, &atlas);
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        // A well-placed glyph beside the poisoned ones, so this says the run
        // survived rather than only that the call returned.
        let _ = canvas.draw_glyphs(
            &[
                PositionedGlyph::new(solid, [16.0, 16.0], rect),
                PositionedGlyph::new(solid, [v, 40.0], rect),
                PositionedGlyph::new(solid, [40.0, v], rect),
            ],
            &atlas,
            0,
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)),
        );

        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        ctx.destroy_image(image);

        assert_eq!(
            pixel(&pixels, 20, 20),
            [255, 0, 0, 255],
            "{name}: the well-placed glyph in the same run did not survive"
        );
    }
}

/// One call made with a value that is not a number, drawn onto a canvas.
type PoisonedDraw = Box<dyn Fn(&mut Canvas)>;

#[gtest]
fn a_draw_placed_at_nan_contributes_nothing() {
    let Some(mut ctx) = context() else { return };
    // Infinity and NaN are not the same request. A caller who writes infinity
    // means the largest thing available, and answering with a shape that covers
    // everything is a defensible reading of it. NaN is not a location, a size,
    // or a width; it is what arithmetic produces when it has already gone
    // wrong, and there is no picture it asks for. So the rule is that such a
    // draw contributes nothing, which is checkable by drawing it beside
    // something valid and requiring the result to be untouched.
    //
    // Checked across the calls rather than one at a time, because a guard is
    // usually added where a bug was found and the calls that were never
    // reported keep whatever they had.
    let reference = |ctx: &mut Context| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)),
            )
            .expect("reference");
        render(ctx, canvas)
    };
    let expected = reference(&mut ctx);

    let n = f32::NAN;
    // Each entry draws the same reference square and then one poisoned call.
    let cases: Vec<(&str, PoisonedDraw)> = vec![
        (
            "rect",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rect(Rect::new(n, n, n, n), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "rect with a nan edge",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rect(Rect::new(0.0, 0.0, n, 64.0), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "rounded rect",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rrect(
                    Rect::new(n, 0.0, 64.0, 64.0),
                    8.0,
                    &Paint::fill(Color::WHITE),
                );
            }),
        ),
        (
            "circle",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_circle(Vec2::new(n, 64.0), 32.0, &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "oval",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_oval(Rect::new(0.0, n, 64.0, 64.0), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "line",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_line(
                    Vec2::new(n, 0.0),
                    Vec2::new(128.0, 128.0),
                    &Paint::stroke(Color::WHITE, 4.0),
                );
            }),
        ),
        (
            "line of nan width",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_line(
                    Vec2::new(0.0, 0.0),
                    Vec2::new(128.0, 128.0),
                    &Paint::stroke(Color::WHITE, n),
                );
            }),
        ),
        (
            "path",
            Box::new(move |c: &mut Canvas| {
                let mut builder = PathBuilder::new();
                builder.move_to(Vec2::new(0.0, 0.0));
                builder.line_to(Vec2::new(n, 64.0));
                builder.line_to(Vec2::new(64.0, 64.0));
                builder.close();
                let _ = c.draw_path(&builder.build(), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "translate",
            Box::new(move |c: &mut Canvas| {
                c.save();
                c.translate(n, 0.0);
                let _ = c.draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE));
                c.restore();
            }),
        ),
        (
            "scale",
            Box::new(move |c: &mut Canvas| {
                c.save();
                c.scale(n, 1.0);
                let _ = c.draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE));
                c.restore();
            }),
        ),
        (
            "rotate",
            Box::new(move |c: &mut Canvas| {
                c.save();
                c.rotate(n);
                let _ = c.draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE));
                c.restore();
            }),
        ),
        (
            "gradient endpoints",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rect(
                    Rect::from_size(64.0, 64.0),
                    &Paint::linear_gradient(
                        Vec2::new(n, n),
                        Vec2::new(n, n),
                        vec![
                            GradientStop::new(Color::WHITE, 0.0),
                            GradientStop::new(Color::BLACK, 1.0),
                        ],
                    ),
                );
            }),
        ),
    ];

    for (name, poison) in &cases {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)),
            )
            .expect("reference");
        poison(&mut canvas);
        let got = render(&mut ctx, canvas);
        let differing = got
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        // Non-fatal, so one bad call does not hide the rest of the sweep. The
        // whole point of asking every drawing call the same question is to
        // learn which of them answer it wrongly, and a fatal assertion reports
        // the first and stops.
        expect_that!(
            differing,
            eq(0),
            "{name} at NaN changed {differing} pixel(s); such a draw should contribute nothing"
        );
    }
}

#[test]
fn a_gradient_with_many_stops_agrees_with_one_that_fits() {
    let Some(mut ctx) = context() else { return };
    // The two paths through the shader must produce the same picture where both
    // can express it. Below the limit the stops travel in push constants and
    // are walked; above it the recorder tabulates them into a texture and the
    // shader reads it. A caller does not choose between those and should not be
    // able to tell which happened.
    //
    // So: the same gradient stated twice. Four stops that fit, and the same
    // ramp restated with extra stops placed exactly on the line between them,
    // which changes nothing about the gradient and everything about how it is
    // carried.
    let ends = vec![
        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.5, 0.0, 0.5, 1.0), 0.5),
        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
    ];
    let mut many = ends.clone();
    for (offset, t) in [(0.25, 0.25f32), (0.75, 0.75)] {
        // Exactly on the line: red to purple to blue is linear in each half.
        let c = if t < 0.5 { t * 2.0 } else { (t - 0.5) * 2.0 };
        let color = if t < 0.5 {
            Color::linear(1.0 - c * 0.5, 0.0, c * 0.5, 1.0)
        } else {
            Color::linear(0.5 - c * 0.5, 0.0, 0.5 + c * 0.5, 1.0)
        };
        many.push(GradientStop::new(color, offset));
    }
    many.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
    assert!(
        ends.len() <= MAX_STOPS,
        "the control must take the walked path"
    );
    assert!(
        many.len() > MAX_STOPS,
        "the subject must take the ramp path"
    );

    let render_with = |ctx: &mut Context, stops: Vec<GradientStop>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::linear_gradient(Vec2::ZERO, Vec2::new(128.0, 0.0), stops)
                    .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    let walked = render_with(&mut ctx, ends);
    let sampled = render_with(&mut ctx, many);

    // Eight bits through a transfer function is not the same arithmetic as
    // interpolating in linear float, so this is a tolerance rather than an
    // equality -- but a small one, and any real disagreement about where a
    // color sits would be far larger than a quantization step.
    let mut worst = 0i32;
    for x in 0..128u32 {
        let a = pixel(&walked, x, 64);
        let b = pixel(&sampled, x, 64);
        for channel in 0..4 {
            worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
        }
    }
    assert!(
        worst <= 3,
        "the two paths disagree by {worst}, which is more than rounding"
    );
    // And the gradient is a gradient rather than two flat halves, which is what
    // a ramp that failed to upload would look like against a black clear.
    let left = pixel(&sampled, 4, 64);
    let right = pixel(&sampled, 124, 64);
    assert!(left[0] > 200 && left[2] < 60, "left end {left:?}");
    assert!(right[2] > 200 && right[0] < 60, "right end {right:?}");
}

#[gtest]
fn a_mask_blur_softens_a_shape_and_matches_a_blurred_layer() {
    let Some(mut ctx) = context() else { return };
    // A mask blur is defined as blurring coverage and then filling. It is
    // implemented as filling into a blurred layer, which is the opposite order
    // — and the two are the same picture only because a blur is linear and the
    // fill is constant: blur(C·a) is C·blur(a) for a constant C.
    //
    // That identity is the whole justification, so it is what gets asserted.
    // The same shape, same sigma, one through the mask blur and one through a
    // layer written out by hand, must agree.
    let sigma = 6.0;
    let shape = Rect::new(40.0, 40.0, 88.0, 88.0);
    let color = Color::linear(1.0, 0.4, 0.1, 1.0);

    let mut masked = Canvas::new(SIZE);
    masked.clear(Color::BLACK);
    masked
        .draw_rect(shape, &Paint::fill(color).with_mask_blur(sigma))
        .expect("mask blur");
    let masked = render(&mut ctx, masked);

    let mut layered = Canvas::new(SIZE);
    layered.clear(Color::BLACK);
    // The same bounds the mask blur computes: the shape, widened by the blur's
    // reach. A tighter bound would cut the tail off square.
    let reach = sigma * 3.0;
    layered.save_layer_bounds(
        Layer::opacity(1.0).with_blur(sigma),
        Rect::new(
            shape.left - reach,
            shape.top - reach,
            shape.right + reach,
            shape.bottom + reach,
        ),
    );
    layered
        .draw_rect(shape, &Paint::fill(color))
        .expect("shape");
    layered.restore();
    let layered = render(&mut ctx, layered);

    let worst = masked
        .iter()
        .zip(&layered)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap_or(0);
    expect_that!(
        worst,
        le(1),
        "a mask blur and a blurred layer of the same shape differ by {worst}"
    );

    // And it actually blurred: the shape's own edge is soft, and light reaches
    // beyond where the shape ends.
    let edge = pixel(&masked, 88, 64);
    expect_true!(
        edge[0] > 8 && edge[0] < 247,
        "the edge is not soft: {edge:?}"
    );
    // And it reaches past where the shape ends, which is checked against the
    // same shape drawn without the blur rather than against a threshold: two
    // sigma out the value is single digits, and a number picked by eye there is
    // a number that passes or fails for reasons about the picking.
    let mut sharp = Canvas::new(SIZE);
    sharp.clear(Color::BLACK);
    sharp
        .draw_rect(shape, &Paint::fill(color).with_anti_alias(false))
        .expect("sharp");
    let sharp = render(&mut ctx, sharp);
    let (beyond, control) = (pixel(&masked, 100, 64), pixel(&sharp, 100, 64));
    expect_that!(
        control[0],
        eq(0),
        "the control should not reach here at all"
    );
    expect_true!(
        beyond[0] > control[0],
        "nothing reached past the shape: {beyond:?} against {control:?}"
    );
}

#[gtest]
fn a_mask_blur_on_a_gradient_is_refused_rather_than_reordered() {
    let Some(mut ctx) = context() else { return };
    // The identity above holds for a constant fill and not otherwise, so a
    // gradient asked for one thing would be given the other. Refusing is the
    // same choice this renderer makes for a blend mode a device cannot do.
    let _ = &mut ctx;
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let refused = canvas.draw_rect(
        Rect::new(16.0, 16.0, 112.0, 112.0),
        &Paint::linear_gradient(
            Vec2::ZERO,
            Vec2::new(128.0, 0.0),
            vec![
                GradientStop::new(Color::WHITE, 0.0),
                GradientStop::new(Color::BLACK, 1.0),
            ],
        )
        .with_mask_blur(4.0),
    );
    expect_true!(
        refused.is_err(),
        "a mask blur on a gradient should be refused"
    );
    // A solid one of the same size is not.
    expect_true!(
        canvas
            .draw_rect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                &Paint::fill(Color::WHITE).with_mask_blur(4.0)
            )
            .is_ok(),
        "a solid mask blur should be accepted"
    );
}

#[test]
fn a_backdrop_blur_softens_what_is_behind_the_layer_and_nothing_else() {
    let Some(mut ctx) = context() else { return };
    // Frosted glass, which is the other blur. A layer blur softens the layer's
    // own content; this softens what is already on the target and hands the
    // result to the layer to draw over.
    //
    // The scene is a hard vertical edge -- black left, white right -- so
    // "blurred" means something checkable: at the edge, a blur produces
    // intermediate values, and no amount of drawing an unblurred copy does.
    let draw = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(64.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("edge");
        // A panel over the middle band only, so the top and bottom of the same
        // edge are left alone and can be compared against.
        canvas.save_layer_bounds(
            Layer::opacity(1.0).with_backdrop_blur(sigma),
            Rect::new(0.0, 48.0, 128.0, 80.0),
        );
        canvas.restore();
        render(ctx, canvas)
    };

    let plain = draw(&mut ctx, 0.0);
    let frosted = draw(&mut ctx, 6.0);

    // Without a backdrop blur the edge is hard: one column black, the next
    // white, nothing between.
    let hard = |pixels: &[u8], y: u32| {
        (56u32..72)
            .map(|x| pixel(pixels, x, y))
            .filter(|p| p[0] > 8 && p[0] < 247)
            .count()
    };
    assert_eq!(hard(&plain, 64), 0, "the control edge is not hard");

    // Inside the panel it is soft.
    let soft = hard(&frosted, 64);
    assert!(
        soft >= 8,
        "the backdrop was not blurred: {soft} intermediate columns at the edge"
    );

    // Outside the panel the same edge is untouched, which is what says the
    // filter is confined to the layer rather than applied to the frame.
    assert_eq!(
        hard(&frosted, 16),
        0,
        "the backdrop blur reached above the layer"
    );
    assert_eq!(
        hard(&frosted, 112),
        0,
        "the backdrop blur reached below the layer"
    );

    // And what was behind is still behind: far from the edge the panel shows
    // the backdrop's own colors rather than clearing them away.
    assert!(
        pixel(&frosted, 4, 64)[0] < 8,
        "the left of the panel is not black"
    );
    assert!(
        pixel(&frosted, 124, 64)[0] > 247,
        "the right of the panel is not white"
    );
}

#[test]
fn a_tint_recolors_an_image_and_keeps_it_premultiplied() {
    let Some(mut ctx) = context() else { return };
    // The reason a tint exists: one monochrome sheet, every state's color. The
    // image here is white at full and half alpha, which is what an antialiased
    // icon's interior and edge look like, and the two together are what catches
    // a tint that multiplies color without accounting for its own alpha.
    let size = Extent2D::new(2, 1);
    let mut icon = vec![0u8; 2 * 4];
    icon[0..4].copy_from_slice(&[255, 255, 255, 255]);
    // Premultiplied: half alpha means half color too.
    icon[4..8].copy_from_slice(&[128, 128, 128, 128]);
    let mut image = ctx
        .create_image(size, PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &icon).expect("upload");

    let draw = |ctx: &mut Context, image: &_, tint: Color, texel: f32| {
        let mut canvas = Canvas::new(SIZE);
        // Transparent rather than opaque black, so what comes back is the
        // source's own premultiplied color. Over an opaque background the
        // destination's alpha survives compositing and the result reads 255
        // whatever the source did, which says nothing about the tint -- the
        // first version of this test asserted on exactly that number.
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::image(0, Rect::from_size(128.0, 128.0))
                    .with_source_pixels(Rect::new(texel, 0.0, texel + 1.0, 1.0), size)
                    .with_tint(tint)
                    .with_anti_alias(false),
            )
            .expect("icon");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixel(&pixels, 64, 64)
    };

    // White is the identity, so an image that never mentions a tint is
    // unchanged by having one.
    let plain = draw(&mut ctx, &image, Color::WHITE, 0.0);
    assert_eq!(
        plain,
        [255, 255, 255, 255],
        "a white tint changed the image"
    );

    // A red tint on the opaque texel gives red at full alpha.
    let red = draw(&mut ctx, &image, Color::linear(1.0, 0.0, 0.0, 1.0), 0.0);
    assert_eq!(red, [255, 0, 0, 255], "an opaque texel tinted red");

    // The same tint on the half-covered texel gives red at half alpha, and --
    // this is the part worth asserting -- its color does not exceed its alpha,
    // which is what premultiplied means and what a tint applied without regard
    // to its own alpha would break.
    let edge = draw(&mut ctx, &image, Color::linear(1.0, 0.0, 0.0, 1.0), 1.0);
    assert!(
        edge[0].abs_diff(128) <= 2 && edge[3].abs_diff(128) <= 2,
        "a tinted half-covered texel came back {edge:?}"
    );
    assert!(
        edge[0] <= edge[3] + 1,
        "the tint left color above alpha, so the result is not premultiplied: {edge:?}"
    );

    // A tint carrying its own alpha scales coverage as well as color, which is
    // what makes it a generalization of the alpha setting rather than a rival.
    let faded = draw(&mut ctx, &image, Color::linear(1.0, 0.0, 0.0, 0.5), 0.0);
    assert!(
        faded[0].abs_diff(128) <= 2 && faded[3].abs_diff(128) <= 2,
        "a half-alpha tint came back {faded:?}"
    );
    ctx.destroy_image(image);
}

#[test]
fn a_sprite_can_be_drawn_from_a_sheet_by_naming_its_texels() {
    let Some(mut ctx) = context() else { return };
    // What a sprite sheet is for, through the API a caller actually uses: they
    // know the size they uploaded, so they state the piece in texels and hand
    // that size over rather than converting by hand.
    //
    // Every texel a different color, and each selection is a single texel. That
    // matters: with blocks of one color, selecting a block passes whether the
    // coordinate is *mapped* into the selection or merely *clamped* to it, and
    // the first version of this test could not tell those apart. Selecting one
    // texel can -- mapping fills the destination with it, clamping shows it
    // beside its neighbor.
    let size = Extent2D::new(4, 4);
    let color_of =
        |x: usize, y: usize| -> [u8; 4] { [(x * 60 + 15) as u8, (y * 60 + 15) as u8, 200, 255] };
    let mut sheet = vec![0u8; 4 * 4 * 4];
    for y in 0..4usize {
        for x in 0..4usize {
            let at = (y * 4 + x) * 4;
            sheet[at..at + 4].copy_from_slice(&color_of(x, y));
        }
    }
    let mut image = ctx
        .create_image(size, PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &sheet).expect("upload");

    let draw = |ctx: &mut Context, image: &_, source: Rect| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::image(0, Rect::from_size(128.0, 128.0))
                    .with_source_pixels(source, size)
                    .with_anti_alias(false),
            )
            .expect("sprite");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    for (x, y) in [(0usize, 0usize), (1, 0), (3, 1), (2, 3)] {
        let source = Rect::new(x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0);
        let pixels = draw(&mut ctx, &image, source);
        let want = color_of(x, y);
        // Across the whole destination, corners included: one texel selected
        // means one color everywhere, and a neighbor appearing anywhere is
        // either a mapping that did not scale or a filter reading past the
        // selection.
        for (px, py) in [(64u32, 64u32), (2, 2), (125, 2), (2, 125), (125, 125)] {
            let got = pixel(&pixels, px, py);
            assert_eq!(
                [got[0], got[1]],
                [want[0], want[1]],
                "texel ({x}, {y}) came back {got:?} at ({px}, {py}), wanted {want:?}"
            );
        }
    }
    // Selecting one texel is not enough on its own: it makes the sampling
    // bounds degenerate, so the coordinate lands on that texel's center whether
    // it was mapped into the selection or merely held inside it. A selection
    // two texels wide separates them, because only a mapping rescales -- the
    // boundary between the two belongs at the middle of the destination, and
    // holding without mapping leaves it at a quarter.
    let pixels = draw(&mut ctx, &image, Rect::new(0.0, 0.0, 2.0, 1.0));
    let first = color_of(0, 0);
    let at_first_center = pixel(&pixels, 32, 64);
    assert_eq!(
        [at_first_center[0], at_first_center[1]],
        [first[0], first[1]],
        "a two-texel selection is not being stretched across the destination"
    );
    let second = color_of(1, 0);
    let at_second_center = pixel(&pixels, 96, 64);
    assert_eq!(
        [at_second_center[0], at_second_center[1]],
        [second[0], second[1]],
        "the second texel of the selection is not where it belongs"
    );
    ctx.destroy_image(image);
}

#[test]
fn an_arc_leaves_the_part_of_the_circle_it_does_not_sweep() {
    let Some(mut ctx) = context() else { return };
    // The end-to-end check that a partial arc is partial. The geometry is
    // covered without a device elsewhere; what this adds is that the builder is
    // reachable from the facade and that a three-quarter sweep reaches the
    // screen as three quarters rather than as a circle.
    let mut builder = PathBuilder::new();
    builder.arc(
        Vec2::new(64.0, 64.0),
        Vec2::splat(40.0),
        // From twelve o'clock, three quarters of the way round.
        -std::f32::consts::FRAC_PI_2,
        std::f32::consts::TAU * 0.75,
    );
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_path(
            &builder.build(),
            &Paint::stroke(Color::WHITE, 8.0).with_anti_alias(false),
        )
        .expect("arc");
    let pixels = render(&mut ctx, canvas);

    // The sweep runs clockwise from twelve and ends at nine, so all four
    // quarter points are on it -- nine is the end of the arc, not past it --
    // and the quarter that is missing lies between nine and twelve. Probing
    // nine expecting nothing is the mistake this comment exists to prevent; it
    // is where the arc stops, which is exactly where it is still drawn.
    for (name, x, y) in [
        ("twelve", 64, 24),
        ("three", 104, 64),
        ("six", 64, 104),
        ("nine", 24, 64),
    ] {
        assert!(
            pixel(&pixels, x, y)[0] > 200,
            "{name} o'clock should be on the arc"
        );
    }
    // Half past ten, in the middle of the quarter that was never swept.
    assert_eq!(
        pixel(&pixels, 36, 36),
        [0, 0, 0, 255],
        "the unswept quarter was drawn"
    );
    // And the middle is empty, so this is a ring rather than a filled shape.
    assert_eq!(pixel(&pixels, 64, 64), [0, 0, 0, 255], "the ring is filled");
}

#[test]
fn a_dashed_stroke_draws_less_than_a_solid_one_and_leaves_real_gaps() {
    let Some(mut ctx) = context() else { return };
    let draw = |ctx: &mut Context, dash: Option<Dash>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_line(
                Vec2::new(0.0, 64.0),
                Vec2::new(128.0, 64.0),
                &Paint::stroke(Color::WHITE, 8.0)
                    .with_dash(dash)
                    .with_anti_alias(false),
            )
            .expect("line");
        render(ctx, canvas)
    };
    let lit = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|texel| texel[0] > 128)
            .count()
    };

    let solid = draw(&mut ctx, None);
    // Sixteen on, sixteen off across a hundred and twenty-eight: four dashes.
    let dashed = draw(&mut ctx, Some(Dash::new(vec![16.0, 16.0], 0.0)));

    let (whole, cut) = (lit(&solid), lit(&dashed));
    assert!(whole > 0, "the solid line drew nothing to compare against");
    // Half, give or take the ends of each dash.
    let ratio = cut as f32 / whole as f32;
    assert!(
        (0.4..0.6).contains(&ratio),
        "a half-on pattern drew {ratio} of the line"
    );

    // The gaps are gaps rather than a dimmer line, which is what a pattern
    // applied to coverage instead of to geometry would produce.
    assert_eq!(
        pixel(&dashed, 24, 64),
        [0, 0, 0, 255],
        "the first gap is not empty"
    );
    assert!(pixel(&dashed, 8, 64)[0] > 200, "the first dash is missing");

    // Phase moves the pattern rather than changing how much is drawn: what was
    // a gap becomes a dash.
    let shifted = draw(&mut ctx, Some(Dash::new(vec![16.0, 16.0], 16.0)));
    assert!(
        pixel(&shifted, 24, 64)[0] > 200,
        "a phase of one interval should put a dash where the gap was"
    );
    assert_eq!(
        pixel(&shifted, 8, 64),
        [0, 0, 0, 255],
        "and a gap where the dash was"
    );
}

#[test]
fn a_dashed_rounded_rectangle_dashes_rather_than_drawing_solid() {
    let Some(mut ctx) = context() else { return };
    // A rounded rectangle stroke is normally evaluated per fragment from a
    // distance field, which describes a continuous outline and has no notion of
    // a position along it. A dash therefore has to send the shape back to
    // tessellation, and if it does not, the outline draws solid and the pattern
    // is silently ignored -- a bug that looks exactly like a working stroke.
    let draw = |ctx: &mut Context, dash: Option<Dash>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                16.0,
                &Paint::stroke(Color::WHITE, 6.0).with_dash(dash),
            )
            .expect("rrect");
        render(ctx, canvas)
    };
    let lit = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|texel| texel[0] > 128)
            .count()
    };

    let solid = lit(&draw(&mut ctx, None));
    let dashed = lit(&draw(&mut ctx, Some(Dash::new(vec![10.0, 10.0], 0.0))));
    assert!(solid > 0, "the solid outline drew nothing");
    let ratio = dashed as f32 / solid as f32;
    assert!(
        (0.35..0.65).contains(&ratio),
        "the dashed outline drew {ratio} of the solid one, so the pattern was ignored"
    );
}

#[test]
fn a_tiled_gradient_tiles_the_same_whether_or_not_its_stops_fit() {
    let Some(mut ctx) = context() else { return };
    // Tiling folds the parameter before anything reads a color, and the two
    // ways of reading one -- walking the stops, sampling a baked ramp -- sit
    // behind that fold. So they should compose, and each was tested without the
    // other, which is the arrangement in which a composition bug survives.
    //
    // The ramp is sampled through a clamp-to-edge sampler, which is the part
    // worth checking rather than assuming: a repeating gradient asks for the
    // last color at the end of each period and the first at the start of the
    // next, and clamping is what gives that instead of blending across the
    // seam. Getting it wrong would show as a soft band at every period, only
    // on the ramp path, only when repeating.
    let ramp_stops = |n: usize| -> Vec<GradientStop> {
        // Collinear, so the picture does not depend on how many there are --
        // only on which path carries them.
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                GradientStop::new(Color::linear(1.0 - t, 0.0, t, 1.0), t)
            })
            .collect()
    };

    let render_with = |ctx: &mut Context, stops: Vec<GradientStop>, tile: TileMode| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("ground");
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                // A ramp spanning a quarter, so three quarters of the picture
                // is whatever the tile mode says.
                &Paint::linear_gradient(Vec2::ZERO, Vec2::new(32.0, 0.0), stops)
                    .with_tile_mode(tile)
                    .with_blend(BlendMode::SrcOver)
                    .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    for tile in [
        TileMode::Clamp,
        TileMode::Repeat,
        TileMode::Mirror,
        TileMode::Decal,
    ] {
        let walked = render_with(&mut ctx, ramp_stops(MAX_STOPS), tile);
        let sampled = render_with(&mut ctx, ramp_stops(MAX_STOPS + 3), tile);
        let mut worst = 0i32;
        for x in 0..128u32 {
            let a = pixel(&walked, x, 64);
            let b = pixel(&sampled, x, 64);
            for channel in 0..4 {
                worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
            }
        }
        assert!(
            worst <= 4,
            "{tile:?}: the two paths disagree by {worst}, which is more than rounding"
        );
    }

    // And the tiling actually happened on the ramp path, rather than both
    // paths agreeing because neither tiled. Two ramps along, repeating is back
    // at the first color where clamping holds the last.
    let repeated = render_with(&mut ctx, ramp_stops(MAX_STOPS + 3), TileMode::Repeat);
    let clamped = render_with(&mut ctx, ramp_stops(MAX_STOPS + 3), TileMode::Clamp);
    let at = pixel(&repeated, 64, 64);
    assert!(
        at[0] > 200 && at[2] < 60,
        "a repeating ramp did not begin again, got {at:?}"
    );
    let held = pixel(&clamped, 64, 64);
    assert!(
        held[2] > 200 && held[0] < 60,
        "a clamped ramp did not hold its end, got {held:?}"
    );
}

#[test]
fn a_gradient_with_many_stops_places_each_one_where_it_was_asked_for() {
    let Some(mut ctx) = context() else { return };
    // The agreement test above uses stops that lie on the line, so it would
    // pass against a ramp that ignored the extra ones entirely. This one cannot:
    // six stops, each a different color, at offsets a truncating implementation
    // would never reach.
    let stops = vec![
        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::linear(1.0, 1.0, 0.0, 1.0), 0.2),
        GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 0.4),
        GradientStop::new(Color::linear(0.0, 1.0, 1.0, 1.0), 0.6),
        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 0.8),
        GradientStop::new(Color::linear(1.0, 0.0, 1.0, 1.0), 1.0),
    ];
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(Vec2::ZERO, Vec2::new(128.0, 0.0), stops)
                .with_anti_alias(false),
        )
        .expect("gradient");
    let pixels = render(&mut ctx, canvas);

    // Each stop sits a fifth of the way along, so each is checked at its own
    // column. The fifth and sixth are the ones a four-stop truncation loses.
    for (name, x, want) in [
        ("red", 0u32, [255u8, 0, 0]),
        ("yellow", 25, [255, 255, 0]),
        ("green", 51, [0, 255, 0]),
        ("cyan", 77, [0, 255, 255]),
        ("blue", 102, [0, 0, 255]),
        ("magenta", 127, [255, 0, 255]),
    ] {
        let got = pixel(&pixels, x, 64);
        for channel in 0..3 {
            assert!(
                (got[channel] as i32 - want[channel] as i32).abs() <= 12,
                "the {name} stop came back {got:?}, wanted about {want:?}"
            );
        }
    }
}

#[test]
fn a_gradient_tiles_beyond_its_own_extent() {
    let Some(mut ctx) = context() else { return };
    // Inside the ramp every mode agrees, which is the control: a difference
    // found outside it is about tiling and not about the gradient.
    let inside = 16;
    let modes = [TileMode::Clamp, TileMode::Repeat, TileMode::Decal];
    let images: Vec<Vec<u8>> = modes.iter().map(|t| tiled_ramp(&mut ctx, *t)).collect();
    for image in &images[1..] {
        for channel in 0..4 {
            assert!(
                (images[0][(inside * 4 + channel) as usize] as i32
                    - image[(inside * 4 + channel) as usize] as i32)
                    .abs()
                    <= 2,
                "the modes disagree inside the ramp, where they should not"
            );
        }
    }

    // Clamp holds the last stop. At x=100 the parameter is past three, and the
    // color there must be the blue the ramp ended on.
    let clamped = pixel(&images[0], 100, 64);
    assert!(
        clamped[2] > 240 && clamped[0] < 16,
        "clamp should hold the end color, got {clamped:?}"
    );

    // Repeat starts the ramp again. x=64 is exactly two ramps along, so it is
    // the red the gradient began with rather than the blue it ended on.
    let repeated = pixel(&images[1], 64, 64);
    assert!(
        repeated[0] > 240 && repeated[2] < 16,
        "repeat should begin the ramp again, got {repeated:?}"
    );
    // And it must still be periodic further out, not merely different once.
    let next_period = pixel(&images[1], 96, 64);
    for channel in 0..4 {
        assert!(
            (repeated[channel] as i32 - next_period[channel] as i32).abs() <= 2,
            "repeat is not periodic: {repeated:?} against {next_period:?}"
        );
    }

    // Decal draws nothing outside, so the white ground shows through.
    let decaled = pixel(&images[2], 100, 64);
    assert!(
        decaled.iter().take(3).all(|c| *c > 240),
        "decal should leave the ground alone, got {decaled:?}"
    );
}

#[test]
fn mirroring_repeats_without_the_seam_that_repeating_leaves() {
    let Some(mut ctx) = context() else { return };
    // The point of mirroring is not that it differs from repeating somewhere --
    // that would be true of getting the fold wrong as well. It is that the
    // copies join. The ramp ends at x=32, so that is where the second copy
    // begins and where a seam would be: repeating jumps from the last color
    // back to the first, and mirroring turns around and runs back.
    let repeated = tiled_ramp(&mut ctx, TileMode::Repeat);
    let mirrored = tiled_ramp(&mut ctx, TileMode::Mirror);

    let jump = |image: &[u8]| {
        let before = pixel(image, 30, 64);
        let after = pixel(image, 34, 64);
        (0..3)
            .map(|c| (before[c] as i32 - after[c] as i32).abs())
            .max()
            .unwrap_or(0)
    };

    let seam = jump(&repeated);
    assert!(
        seam > 200,
        "repeating should jump at the period boundary, but moved only {seam}"
    );
    let joined = jump(&mirrored);
    assert!(
        joined < 40,
        "mirroring should join at the period boundary, but jumped {joined}"
    );

    // And it is a reflection rather than a hold: a quarter past the boundary
    // must match a quarter before it, which clamping would also fail.
    //
    // The two columns are 24 and 39 rather than 24 and 40, because a fragment
    // samples at its center: 24.5 reflects about the boundary at 32 onto 39.5,
    // and 40.5 is a whole pixel further out. Reading them as 24 and 40 leaves a
    // difference of eight in the blue channel, which is the ramp's slope over
    // one pixel and not a fault in the fold.
    let before = pixel(&mirrored, 24, 64);
    let after = pixel(&mirrored, 39, 64);
    for channel in 0..3 {
        assert!(
            (before[channel] as i32 - after[channel] as i32).abs() <= 6,
            "the second copy should mirror the first: {before:?} against {after:?}"
        );
    }
}

#[test]
fn a_partial_sweep_holds_its_end_color_around_the_rest_of_the_turn() {
    let Some(mut ctx) = context() else { return };
    // The only case where a sweep has an outside at all. A full turn covers
    // every direction, so tiling it changes nothing and would make a test that
    // passes whatever the code does.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::sweep_gradient(
                Vec2::new(64.0, 64.0),
                0.0,
                std::f32::consts::PI,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("sweep");
    let pixels = render(&mut ctx, canvas);

    // The arc runs from the positive X axis through the bottom of the image to
    // the negative X axis: the renderer's Y points down, so increasing angle
    // goes that way. Measured rather than assumed -- the first version of this
    // test probed a direction it took to be outside, which was inside, and
    // passed against an implementation that had no clamping at all.
    let start = pixel(&pixels, 64 + 40, 64);
    assert!(
        start[0] > 240 && start[2] < 16,
        "the arc should begin at the first stop, got {start:?}"
    );
    let middle = pixel(&pixels, 64, 64 + 40);
    assert!(
        middle[0] > 100 && middle[0] < 160 && middle[2] > 100 && middle[2] < 160,
        "half way round the arc should be half way along the ramp, got {middle:?}"
    );

    // Straight up is outside the arc, and is the direction that tells the two
    // possible implementations apart. Clamping holds the last stop, so it is
    // pure blue. Folding the angle by the sweep instead of by a whole turn --
    // which is what this did before -- runs the ramp a second time around the
    // circle and puts the halfway color here instead.
    let outside = pixel(&pixels, 64, 64 - 40);
    assert!(
        outside[2] > 240 && outside[0] < 16,
        "outside the arc should hold the end color, got {outside:?}"
    );
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

    // Clip Y runs up, so a point below the center in the image is at a negative
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
    // One byte per texel, which is what an atlas holds. The shader reads the
    // red channel either way, so a four-channel atlas would work and cost four
    // times the memory and four times the bandwidth to sample.
    let mut image = ctx
        .create_image(
            Extent2D::new(atlas.size(), atlas.size()),
            PixelFormat::R8Unorm,
        )
        .expect("atlas image");
    ctx.write_image(&mut image, atlas.texels()).expect("upload");
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
fn a_glyph_run_is_correct_after_the_atlas_has_grown() {
    let Some(mut ctx) = context() else { return };

    // Growth is the other answer to a full atlas, and it breaks more than
    // compaction does. Compaction moves glyphs within a texture that stays the
    // same size; growth doubles the texture, so every texture coordinate
    // divides by a different number and the image the glyphs were uploaded
    // into is the wrong shape to hold them. The test next to this one
    // deliberately pins its atlas so it cannot grow, which left the doubling
    // path with no end-to-end coverage at all.
    //
    // The property is that none of it is visible: a glyph drawn from a grown
    // atlas lands exactly where the same glyph drawn from a small one did.
    let marker = GlyphKey {
        font: 1,
        glyph: 1,
        size: 16,
    };
    // Distinguishable from the filler below, so a run reading the wrong
    // rectangle draws a visibly different gray rather than a similar one.
    let coverage = Coverage {
        width: 6,
        height: 6,
        texels: vec![255; 36],
    };

    let draw = |ctx: &mut Context, atlas: &Atlas| {
        let image = upload_atlas(ctx, atlas);
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_glyphs(
                &[PositionedGlyph::new(
                    marker,
                    [40.0, 40.0],
                    atlas.get(marker).unwrap(),
                )],
                atlas,
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
        pixels
    };

    // Before: a small atlas holding just the marker.
    let mut small = Atlas::new(32);
    small.insert(marker, &coverage).expect("marker");
    let before = draw(&mut ctx, &small);

    // After: the same marker in an atlas pushed past its starting size. Every
    // glyph is wanted this frame, so there is nothing stale to evict and
    // growing is the only way to fit them.
    let mut grown = Atlas::new(32);
    grown.insert(marker, &coverage).expect("marker");
    let mut filler = 2u16;
    while grown.growths() == 0 {
        grown
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
            .expect("should grow rather than refuse");
        filler += 1;
        assert!(filler < 400, "the atlas never grew");
    }
    assert!(grown.size() > small.size(), "the atlas did not get bigger");
    assert!(grown.is_dirty(), "a grown atlas owes an upload");
    assert_eq!(
        grown.compactions(),
        0,
        "this is about growth, not repacking"
    );

    let after = draw(&mut ctx, &grown);

    // Identical, not merely similar. The glyph is the same texels at the same
    // place on screen, and everything that changed was the atlas's own
    // business -- which is the whole claim.
    let worst = before
        .iter()
        .zip(&after)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert_eq!(
        worst, 0,
        "growing the atlas moved or altered the glyph on screen"
    );
    // And the glyph was actually drawn, so this is not two black frames.
    assert!(
        before.iter().any(|&b| b > 128),
        "the marker did not render at all"
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
    // Fixed at its starting size, because this is about compaction: an atlas
    // free to grow answers a full one by doubling and never repacks.
    let mut atlas = Atlas::with_limit(32, 32);
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

/// Render a scene twice — once with its layer told what it covers and once
/// not — and require the two to agree.
///
/// Bounds are an optimization, so this is the whole guarantee: the pixels must
/// not move. Checking against the unbounded path rather than against stored
/// values means it keeps holding as both change, and it catches the failure
/// this feature actually has, which is content landing in the wrong place
/// because two mappings disagreed about where the target is.
fn bounds_are_invisible(ctx: &mut Context, build: impl Fn(&mut Canvas, Option<Rect>)) {
    let record = |bounds: Option<Rect>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        build(&mut canvas, bounds);
        canvas.finish()
    };
    let draw = |ctx: &mut Context, recording: &_| {
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let bounded_recording = record(Some(LAYER_BOUNDS));
    assert!(
        bounded_recording
            .passes
            .iter()
            .any(|p| p.extent.width < SIZE.width || p.extent.height < SIZE.height),
        "no pass got a smaller target, so this compares two identical paths"
    );

    let full = draw(ctx, &record(None));
    let bounded = draw(ctx, &bounded_recording);
    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "a bounded layer differs from a full-size one by {worst}"
    );
    // And the scene drew something, so this is not a comparison of two black
    // frames that would pass however broken the feature was.
    assert!(full.iter().any(|&b| b > 32), "the scene rendered nothing");
}

/// The region the layer scenes below confine themselves to.
const LAYER_BOUNDS: Rect = Rect {
    left: 24.0,
    top: 40.0,
    right: 96.0,
    bottom: 104.0,
};

/// Open a layer with or without bounds, so a scene reads the same either way.
fn open_layer(canvas: &mut Canvas, bounds: Option<Rect>, layer: Layer) {
    match bounds {
        Some(b) => canvas.save_layer_bounds(layer, b),
        None => canvas.save_layer(layer),
    };
}

/// Something worth compositing: a gradient with a translucent shape over it.
///
/// Translucent so that the layer's alpha channel matters, which is where a
/// mistake in the composite mapping shows up as a halo rather than as an
/// offset.
fn layer_contents(canvas: &mut Canvas) {
    canvas
        .draw_rect(
            LAYER_BOUNDS,
            &Paint::linear_gradient(
                Vec2::new(LAYER_BOUNDS.left, LAYER_BOUNDS.top),
                Vec2::new(LAYER_BOUNDS.right, LAYER_BOUNDS.bottom),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.2, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.4, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");
    canvas
        .draw_circle(
            Vec2::new(60.0, 72.0),
            26.0,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 0.7)).with_anti_alias(false),
        )
        .expect("circle");
}

/// The region the blur scenes draw into.
const BLUR_REGION: Rect = Rect {
    left: 40.0,
    top: 40.0,
    right: 88.0,
    bottom: 88.0,
};

/// A white square in a layer, blurred by `sigma`, optionally bounded.
fn blurred_square(ctx: &mut Context, sigma: f32, bounds: Option<Rect>) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let layer = Layer::default().with_blur(sigma);
    match bounds {
        Some(region) => canvas.save_layer_bounds(layer, region),
        None => canvas.save_layer(layer),
    };
    canvas
        .draw_rect(
            BLUR_REGION,
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("square");
    canvas.restore();
    render(ctx, canvas)
}

#[test]
fn a_blurred_layer_spreads_beyond_the_shape_it_holds() {
    let Some(mut ctx) = context() else { return };
    // What a blur is for, stated as the thing that distinguishes it: light
    // where the shape is not. A sharp layer has nothing outside the square.
    let sharp = blurred_square(&mut ctx, 0.0, None);
    let soft = blurred_square(&mut ctx, 8.0, None);
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize];

    assert_eq!(at(&sharp, 64, 34), 0, "a sharp layer stops at its edge");
    assert!(
        at(&soft, 64, 34) > 32,
        "a blurred layer should reach past its shape, got {}",
        at(&soft, 64, 34)
    );
    // And it falls off with distance rather than being a uniform smear, which
    // is what a Gaussian means and what a box filter would not give.
    let near = at(&soft, 64, 36);
    let far = at(&soft, 64, 28);
    assert!(
        near > far && far > 0,
        "the blur should fall off with distance, got {near} then {far}"
    );
    // The middle of a shape much larger than the blur is untouched, so the
    // weights sum to one rather than darkening what they average.
    assert_eq!(at(&soft, 64, 64), 255, "the middle should be undimmed");
}

#[test]
fn a_larger_sigma_spreads_further() {
    let Some(mut ctx) = context() else { return };
    // The parameter does what it says, monotonically. A blur that ignored
    // sigma, or scaled it by the wrong thing, would still pass the test above.
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] as i32;
    let small = blurred_square(&mut ctx, 3.0, None);
    let large = blurred_square(&mut ctx, 12.0, None);
    let probe = (64, 30);
    assert!(
        at(&large, probe.0, probe.1) > at(&small, probe.0, probe.1),
        "a larger sigma should reach further"
    );
    // And spends the light it moved rather than adding any: brighter outside
    // has to come from dimmer inside. Measured well inside the edge, since the
    // two profiles cross over within a pixel of it -- at the boundary itself
    // both are near half and which is greater says nothing.
    assert!(
        at(&large, 64, 46) < at(&small, 64, 46),
        "spreading further should dim the inside it came from"
    );
}

#[test]
fn a_blur_of_zero_records_no_extra_passes() {
    // The passes are only recorded where a blur was asked for, so a caller
    // animating one to nothing pays nothing at the end of the animation.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer(Layer::default());
    canvas
        .draw_rect(BLUR_REGION, &Paint::fill(Color::WHITE))
        .expect("square");
    canvas.restore();
    let plain = canvas.finish().passes.len();

    for sigma in [0.0, -3.0, f32::NAN] {
        let mut canvas = Canvas::new(SIZE);
        canvas.save_layer(Layer::default().with_blur(sigma));
        canvas
            .draw_rect(BLUR_REGION, &Paint::fill(Color::WHITE))
            .expect("square");
        canvas.restore();
        assert_eq!(
            canvas.finish().passes.len(),
            plain,
            "a blur of {sigma} should record no extra passes"
        );
    }
}

#[test]
fn a_blur_keeps_softening_past_the_tap_budget() {
    let Some(mut ctx) = context() else { return };
    // A shader loop is bounded, so past some sigma the taps stop covering
    // three deviations. Truncated there, the blur stops getting softer however
    // large sigma grows and the shape creeps toward a box -- which reads as a
    // blur that has a maximum, and is the wrong answer at exactly the sizes a
    // frosted panel or a large shadow asks for.
    //
    // The taps spread instead, so they still span the curve and the sampler's
    // bilinear filter averages what falls between them. That trades quality
    // for coverage, which is the right way round: a slightly under-sampled
    // wide blur looks like a wide blur, and a truncated one does not.
    let extent = Extent2D::new(256, 256);
    let render = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas.save_layer(Layer::default().with_blur(sigma));
        canvas
            .draw_rect(
                Rect::new(96.0, 96.0, 160.0, 160.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("square");
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * 256 + x) * 4) as usize] as i32;

    // Well past where three deviations stop fitting in the budget, which is a
    // little under eleven at thirty-two taps each way.
    let sigmas = [16.0f32, 32.0, 60.0];
    let rendered: Vec<Vec<u8>> = sigmas.iter().map(|s| render(&mut ctx, *s)).collect();

    for pair in rendered.windows(2) {
        // Further out keeps getting brighter: the blur is still spreading.
        assert!(
            at(&pair[1], 128, 56) > at(&pair[0], 128, 56),
            "a larger sigma stopped reaching further"
        );
        // And the middle keeps getting dimmer, so the light is being moved
        // rather than added.
        assert!(
            at(&pair[1], 128, 128) < at(&pair[0], 128, 128),
            "a larger sigma stopped dimming the middle"
        );
    }
}

#[test]
fn blurred_layers_nest() {
    let Some(mut ctx) = context() else { return };
    // Each blurred layer files three passes and the composite has to name the
    // last of its own, not the last filed. With two nested, an index that
    // counted from the wrong end would composite the inner layer's contents
    // where its blur belongs -- a sharper picture rather than an error.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer(Layer::default().with_blur(4.0));
    canvas.save_layer(Layer::opacity(0.8).with_blur(3.0));
    canvas
        .draw_rect(
            BLUR_REGION,
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("square");
    canvas.restore();
    canvas.restore();
    let recording = canvas.finish();

    // Three per blurred layer, plus the root.
    assert_eq!(
        recording.passes.len(),
        7,
        "two blurred layers and a root should be seven passes"
    );

    let nested = render_recording(&mut ctx, &recording);
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] as i32;
    // Blurred twice, so it reaches further than either alone would.
    let once = blurred_square(&mut ctx, 4.0, None);
    assert!(
        at(&nested, 64, 28) > at(&once, 64, 28),
        "two blurs should reach further than one"
    );
    // And the inner layer's alpha survived the composite rather than being
    // lost when the extra passes were inserted.
    assert!(
        at(&nested, 64, 64) < 255,
        "the inner layer's opacity should still apply"
    );
}

#[test]
fn a_bounded_blurred_layer_matches_a_full_size_one() {
    let Some(mut ctx) = context() else { return };
    // Bounds stay invisible with a blur, which takes more than sizing the
    // target to what the caller said: a blur reaches past its content, so a
    // target sized to the content alone cuts the halo off square at the bound.
    // The caller states where the content is, which is the question they can
    // answer; how far the blur carries it is this renderer's arithmetic.
    let full = blurred_square(&mut ctx, 8.0, None);
    let bounded = blurred_square(&mut ctx, 8.0, Some(BLUR_REGION));

    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert_eq!(
        worst, 0,
        "bounding a blurred layer changed the picture by {worst}"
    );
    // And it did bound something, or this compares two identical recordings.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer_bounds(Layer::default().with_blur(8.0), BLUR_REGION);
    canvas
        .draw_rect(BLUR_REGION, &Paint::fill(Color::WHITE))
        .expect("square");
    canvas.restore();
    let recording = canvas.finish();
    assert!(
        recording.passes[0].extent.width < SIZE.width,
        "the layer should still have gotten a smaller target"
    );
    // Larger than the content by the blur's reach, not equal to it.
    assert!(
        recording.passes[0].extent.width > BLUR_REGION.width() as u32,
        "the target should be outset past the content it holds"
    );
}

#[test]
fn a_bounded_layer_renders_the_same_as_a_full_size_one() {
    let Some(mut ctx) = context() else { return };
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.6));
        layer_contents(canvas);
        canvas.restore();
    });
}

#[test]
fn a_bounded_layer_renders_the_same_under_a_transform() {
    let Some(mut ctx) = context() else { return };
    // The bounds are stated in user space, so the transform in force when the
    // layer opens has to be applied to them. Left out, the target lands
    // somewhere else and the contents are cut off by its edges.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        canvas.save();
        canvas.translate(12.0, -8.0);
        canvas.scale(0.75, 0.5);
        open_layer(canvas, bounds, Layer::opacity(0.6));
        layer_contents(canvas);
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn a_bounded_layer_renders_the_same_under_a_rotation() {
    let Some(mut ctx) = context() else { return };
    // A rotation leaves the region a quadrilateral, and the target is the box
    // around it. That is bigger than the region, which is the safe direction
    // for an allocation and would be the wrong one for a clip.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        canvas.save();
        canvas.translate(64.0, 72.0);
        canvas.rotate(0.4);
        canvas.translate(-64.0, -72.0);
        open_layer(canvas, bounds, Layer::opacity(0.6));
        layer_contents(canvas);
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn bounded_layers_nest() {
    let Some(mut ctx) = context() else { return };
    // An inner layer's target is placed within its parent's, not within the
    // frame's. Compositing it as though the parent filled the frame puts it
    // off by the parent's own offset, which is what this catches.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.8));
        layer_contents(canvas);
        open_layer(
            canvas,
            bounds.map(|_| Rect::new(40.0, 56.0, 88.0, 96.0)),
            Layer::opacity(0.5),
        );
        canvas
            .draw_rect(
                Rect::new(40.0, 56.0, 88.0, 96.0),
                &Paint::fill(Color::linear(0.1, 1.0, 0.3, 1.0)).with_anti_alias(false),
            )
            .expect("inner");
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn a_clip_inside_a_bounded_layer_lands_where_it_would_have() {
    let Some(mut ctx) = context() else { return };
    // A scissor is stated in device pixels and a bounded target starts
    // somewhere other than the origin, so the two have to be reconciled. A
    // stencil clip is a draw and goes through the same mapping the contents
    // do; both are exercised here, since only the rectangle takes the scissor
    // path.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.9));
        canvas.save();
        canvas
            .clip_rect(Rect::new(32.0, 48.0, 84.0, 96.0))
            .expect("scissor");
        let mut triangle = PathBuilder::new();
        triangle
            .move_to(Vec2::new(60.0, 44.0))
            .line_to(Vec2::new(92.0, 100.0))
            .line_to(Vec2::new(28.0, 100.0))
            .close();
        canvas.clip_path(&triangle.build()).expect("stencil");
        layer_contents(canvas);
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn an_antialiased_bounded_layer_renders_the_same_as_a_full_size_one() {
    let Some(mut ctx) = context() else { return };
    // Multisampling resolves per target, so a layer with its own smaller target
    // resolves separately from the frame. The composite that brings it back
    // must not soften its own edges on the way.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.7));
        canvas
            .draw_circle(
                Vec2::new(60.0, 72.0),
                28.0,
                &Paint::fill(Color::linear(0.9, 0.4, 0.1, 1.0)),
            )
            .expect("circle");
        canvas.restore();
    });
}

#[test]
fn glyphs_and_images_in_a_bounded_layer_land_where_they_would_have() {
    let Some(mut ctx) = context() else { return };
    // A glyph run builds its own quads and carries per-vertex texture
    // coordinates, and an image paint maps clip space back to the image
    // itself; neither goes through the path that fills a shape. Both derive
    // their mapping from where the target is, so both need covering here.
    let (atlas, solid, _) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);
    let rect = atlas.get(solid).unwrap();

    let record = |bounds: Option<Rect>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        open_layer(&mut canvas, bounds, Layer::opacity(0.75));
        canvas
            .draw_rect(
                Rect::new(28.0, 44.0, 92.0, 76.0),
                &Paint::image(0, Rect::new(28.0, 44.0, 92.0, 76.0)).with_anti_alias(false),
            )
            .expect("image");
        canvas
            .draw_glyphs(
                &[
                    PositionedGlyph::new(solid, [32.0, 80.0], rect),
                    PositionedGlyph::new(solid, [60.0, 80.0], rect),
                ],
                &atlas,
                0,
                &Paint::fill(Color::WHITE),
            )
            .expect("glyphs");
        canvas.restore();
        canvas.finish()
    };
    let mut draw = |recording: &_| {
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, recording, &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let bounded_recording = record(Some(LAYER_BOUNDS));
    assert!(bounded_recording.passes[0].extent.width < SIZE.width);
    let full = draw(&record(None));
    let bounded = draw(&bounded_recording);
    ctx.destroy_image(image);

    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "glyphs or the image moved by {worst}");
    assert!(full.iter().any(|&b| b > 32), "the scene rendered nothing");
}

#[test]
fn content_outside_a_layers_bounds_is_clipped_rather_than_trusted() {
    let Some(mut ctx) = context() else { return };
    // Bounds are the caller's promise about what the layer covers, and this is
    // what happens when the promise is wrong. The target's own edges cut the
    // drawing off, which is visible and points at the layer that understated
    // itself. The alternative -- believing the caller and sampling a region
    // the target does not have -- reads whatever the allocator last left
    // there, which looks like a driver fault and is a different bug every run.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    // Half as tall as the shape that gets drawn into it.
    canvas.save_layer_bounds(Layer::default(), Rect::new(32.0, 32.0, 96.0, 64.0));
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 96.0, 96.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 64, 48),
        [255, 255, 255, 255],
        "inside the bounds the shape is drawn"
    );
    assert_eq!(
        pixel(&pixels, 64, 80),
        [0, 0, 0, 255],
        "outside the bounds it is cut off, not sampled from somewhere else"
    );
}

#[test]
fn a_layer_gets_a_target_the_size_of_its_bounds() {
    // The point of the feature, stated as the number it is meant to change.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer_bounds(Layer::default(), Rect::new(24.0, 40.0, 96.0, 104.0));
    canvas
        .draw_rect(
            Rect::new(24.0, 40.0, 96.0, 104.0),
            &Paint::fill(Color::WHITE),
        )
        .expect("rect");
    canvas.restore();
    let recording = canvas.finish();
    assert_eq!(recording.passes[0].extent, Extent2D::new(72, 64));
    assert_eq!(
        recording.root().extent,
        SIZE,
        "the root is the caller's surface"
    );
}

#[test]
fn fractional_bounds_are_rounded_outward() {
    // Rounding inward would drop the coverage of the pixels the bound falls
    // inside, which is drawing lost to arithmetic.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer_bounds(Layer::default(), Rect::new(24.3, 40.9, 95.1, 103.2));
    canvas
        .draw_rect(Rect::from_size(128.0, 128.0), &Paint::fill(Color::WHITE))
        .expect("rect");
    canvas.restore();
    let recording = canvas.finish();
    assert_eq!(recording.passes[0].extent, Extent2D::new(72, 64));
}

#[test]
fn bounds_that_cannot_be_honored_fall_back_to_a_full_size_layer() {
    // Every way of ending up without a usable region has to produce a working
    // full-size layer rather than a smaller wrong one or a panic. A smaller
    // target here would be a guess, and guessing wrong loses drawing.
    fn layer_extent(open: impl FnOnce(&mut Canvas)) -> Extent2D {
        let mut canvas = Canvas::new(SIZE);
        open(&mut canvas);
        canvas
            .draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE))
            .expect("rect");
        canvas.restore();
        canvas.finish().passes[0].extent
    }

    // A region with no area, and one entirely outside the frame, both leave
    // nothing to allocate.
    let empty = layer_extent(|canvas| {
        canvas.save_layer_bounds(Layer::default(), Rect::new(50.0, 50.0, 50.0, 50.0));
    });
    assert_eq!(empty, SIZE, "empty bounds");

    let offscreen = layer_extent(|canvas| {
        canvas.save_layer_bounds(Layer::default(), Rect::new(400.0, 400.0, 500.0, 500.0));
    });
    assert_eq!(offscreen, SIZE, "bounds outside the frame");
}

/// Stops for the conical tests: red at the first circle, blue at the second.
fn conical_stops() -> Vec<GradientStop> {
    vec![
        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
    ]
}

#[test]
fn a_conical_gradient_whose_first_circle_is_a_point_at_the_centre_is_a_radial_gradient() {
    // The two take entirely different routes -- the radial folds its radius
    // into a matrix and measures a length, the conical solves a quadratic --
    // and this is the one configuration where they must agree exactly. It is
    // the only check available that compares the new arithmetic against
    // something already known to be right, rather than against my expectation
    // of what the picture should look like.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::radial_gradient(Vec2::new(64.0, 64.0), 60.0, conical_stops())
                .with_anti_alias(false),
        )
        .expect("radial");
    let radial = render(&mut ctx, canvas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(64.0, 64.0),
                0.0,
                Vec2::new(64.0, 64.0),
                60.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let conical = render(&mut ctx, canvas);

    let mut worst = 0i32;
    let mut worst_at = (0, 0);
    for y in 0..128 {
        for x in 0..128 {
            let (a, b) = (pixel(&radial, x, y), pixel(&conical, x, y));
            for channel in 0..4 {
                let delta = (a[channel] as i32 - b[channel] as i32).abs();
                if delta > worst {
                    worst = delta;
                    worst_at = (x, y);
                }
            }
        }
    }
    let (x, y) = worst_at;
    assert!(
        worst <= 1,
        "a conical gradient from a point differs from the radial gradient it is: \
         {worst} at ({x}, {y}), radial {:?} against conical {:?}",
        pixel(&radial, x, y),
        pixel(&conical, x, y)
    );
}

#[test]
fn a_conical_gradient_between_concentric_circles_holds_the_first_colour_inside_the_inner_one() {
    // The configuration that gives the first radius somewhere to show. With
    // both circles on the same center the family is an annulus, the parameter
    // is zero on the inner circle and one on the outer, and everything inside
    // the inner circle is before the gradient starts.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(64.0, 64.0),
                20.0,
                Vec2::new(64.0, 64.0),
                60.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let pixels = render(&mut ctx, canvas);

    // Flat across the hole, which is what distinguishes this from a radial
    // gradient of radius sixty: that one would already be a third of the way
    // to blue at the inner circle.
    let middle = pixel(&pixels, 64, 64);
    let inner_edge = pixel(&pixels, 64 + 18, 64);
    assert!(
        middle[0] > 240 && middle[2] < 16,
        "the hole should hold the first stop, got {middle:?}"
    );
    for channel in 0..4 {
        assert!(
            (middle[channel] as i32 - inner_edge[channel] as i32).abs() <= 2,
            "the hole is not flat: center {middle:?} against {inner_edge:?}"
        );
    }

    // And halfway between the two circles is halfway along the ramp.
    let halfway = pixel(&pixels, 64 + 40, 64);
    assert!(
        halfway[0] > 90 && halfway[0] < 165 && halfway[2] > 90 && halfway[2] < 165,
        "midway between the circles should be midway along the ramp, got {halfway:?}"
    );
}

#[test]
fn every_point_on_the_second_circle_reaches_the_last_stop_wherever_the_first_sits() {
    // What makes a gradient conical rather than radial: the parameter is one
    // on the whole of the second circle, however far off center the first one
    // is. A solve that measured distance from the first circle instead would
    // run from a quarter to three quarters of the ramp around this circle, so
    // the four samples disagreeing is the signature of exactly that mistake.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(44.0, 64.0),
                0.0,
                Vec2::new(64.0, 64.0),
                40.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let pixels = render(&mut ctx, canvas);

    let focus = pixel(&pixels, 44, 64);
    assert!(
        focus[0] > 240 && focus[2] < 16,
        "the first circle collapsed to a point should be the first stop, got {focus:?}"
    );

    let on_circle = [
        pixel(&pixels, 64 + 39, 64),
        pixel(&pixels, 64 - 39, 64),
        pixel(&pixels, 64, 64 + 39),
        pixel(&pixels, 64, 64 - 39),
    ];
    for sample in on_circle {
        assert!(
            sample[2] > 235 && sample[0] < 24,
            "a point on the second circle should be the last stop, got {sample:?}"
        );
    }
}

#[test]
fn a_conical_gradient_whose_circles_are_tangent_draws_only_the_half_plane_it_spans() {
    // The case the quadratic degenerates in: when the distance between the
    // centers equals the difference of the radii, the first circle sits on the
    // second and the squared term vanishes. Solving it as a quadratic anyway
    // divides by zero and loses the single root that does exist, so this is
    // the test for the branch that handles it -- and the branch is not
    // reachable by any nearby configuration, which is why it needs its own.
    //
    // Geometrically the cone opens into a half plane: every circle of the
    // family passes through the first center, so nothing behind it is on any
    // of them, and nothing is drawn there.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(44.0, 64.0),
                0.0,
                Vec2::new(64.0, 64.0),
                20.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let pixels = render(&mut ctx, canvas);

    // Diametrically opposite on the second circle, both a full turn of the
    // ramp away from the first center, and both the last stop.
    for (x, y) in [(84, 64), (64, 84)] {
        let sample = pixel(&pixels, x, y);
        assert!(
            sample[2] > 235 && sample[0] < 24,
            "({x}, {y}) is on the second circle and should be the last stop, got {sample:?}"
        );
    }

    // Behind the first center there is no circle of the family at all. Not a
    // clamped parameter, which would paint the first color across half the
    // rectangle: no parameter.
    for (x, y) in [(20, 64), (4, 4), (4, 124)] {
        let sample = pixel(&pixels, x, y);
        assert_eq!(
            sample,
            [0, 0, 0, 255],
            "({x}, {y}) is on no circle of the family and should be untouched"
        );
    }
}

#[test]
fn a_mesh_draws_the_triangles_it_names_and_nothing_else() {
    // The only geometry here that this renderer did not produce itself, so the
    // thing worth checking is that it is drawn as given: the diagonal of a
    // square split into two triangles is inside both, and a corner cut off by
    // omitting a triangle stays empty.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);

    // One triangle of a square: (16,16), (112,16), (16,112). The far corner is
    // outside it.
    let mesh = Vertices::new(
        VertexMode::Triangles,
        vec![
            Vec2::new(16.0, 16.0),
            Vec2::new(112.0, 16.0),
            Vec2::new(16.0, 112.0),
        ],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 24, 24),
        [255, 255, 255, 255],
        "inside the triangle"
    );
    assert_eq!(
        pixel(&pixels, 100, 100),
        [0, 0, 0, 255],
        "the corner the triangle does not cover"
    );
}

#[test]
fn a_mesh_travels_through_the_canvas_transform() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.translate(64.0, 0.0);
    let mesh = Vertices::new(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 16.0),
            Vec2::new(48.0, 16.0),
            Vec2::new(0.0, 112.0),
        ],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 72, 24),
        [255, 255, 255, 255],
        "the mesh should have moved with the transform"
    );
    assert_eq!(
        pixel(&pixels, 8, 24),
        [0, 0, 0, 255],
        "the mesh should not still be where it was written"
    );
}

#[test]
fn a_mesh_reads_the_texture_where_its_coordinates_say_rather_than_where_it_sits() {
    // The whole reason texture coordinates exist here. The quad covers the
    // target exactly as the image test's rectangle does, but names its
    // coordinates mirrored, so every quadrant must come out on the opposite
    // side from the positional draw. Sampling by position instead would
    // produce the unmirrored picture, which is what makes this test able to
    // tell the two apart at all.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let corners = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(128.0, 0.0),
        Vec2::new(128.0, 128.0),
        Vec2::new(0.0, 128.0),
    ];
    let mirrored = vec![
        Vec2::new(1.0, 0.0),
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0, 1.0),
        Vec2::new(1.0, 1.0),
    ];
    let mesh = Vertices::indexed(
        VertexMode::Triangles,
        corners,
        mirrored,
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_vertices(&mesh, &Paint::image(0, Rect::from_size(128.0, 128.0)))
        .expect("mesh");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    for (x, y, want, corner) in [
        (16u32, 16u32, [0u8, 255, 0, 255], "top-left"),
        (112, 16, [255, 0, 0, 255], "top-right"),
        (16, 112, [255, 255, 0, 255], "bottom-left"),
        (112, 112, [0, 0, 255, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {corner} corner read the wrong texel; the coordinates were ignored"
        );
    }
}

#[test]
fn texture_coordinates_without_a_texture_are_refused_rather_than_dropped() {
    let mut canvas = Canvas::new(SIZE);
    let mesh = Vertices::textured(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            Vec2::new(0.0, 64.0),
        ],
        vec![Vec2::ZERO, Vec2::X, Vec2::Y],
    )
    .expect("mesh");
    assert!(
        canvas
            .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
            .is_err(),
        "a solid paint has no texture for the coordinates to read"
    );
}

#[test]
fn a_source_rectangle_on_a_textured_mesh_is_refused_rather_than_applied_twice() {
    // Two answers to one question. Silently ignoring the rectangle would put a
    // coordinate of one at the texture's edge instead of the sprite's, which
    // looks like a texture that failed to load rather than like a rule.
    let mut canvas = Canvas::new(SIZE);
    let mesh = Vertices::textured(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            Vec2::new(0.0, 64.0),
        ],
        vec![Vec2::ZERO, Vec2::X, Vec2::Y],
    )
    .expect("mesh");
    let paint =
        Paint::image(0, Rect::from_size(64.0, 64.0)).with_source(Rect::new(0.0, 0.0, 0.5, 0.5));
    assert!(
        canvas.draw_vertices(&mesh, &paint).is_err(),
        "a source rectangle and per-vertex coordinates both select part of the texture"
    );
}

/// Four sprites, one per quadrant of the four-by-four fixture, laid out in a
/// different order than they sit in the sheet.
///
/// Shuffled deliberately: a batch that drew each sprite where it sits in the
/// sheet would pass a test that only checked colors were present. Moving them
/// means each destination names exactly one source, and getting the mapping
/// backwards puts the wrong color in a place a reader can see.
fn shuffled_quadrants() -> Vec<Sprite> {
    let quarter = |x, y| SourceRect::new(x, y, 2.0, 2.0);
    vec![
        // Bottom-right of the sheet (yellow) into the top-left of the target.
        Sprite::new(
            quarter(2.0, 2.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(0.0, 0.0)),
        ),
        // Bottom-left (blue) into the top-right.
        Sprite::new(
            quarter(0.0, 2.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(64.0, 0.0)),
        ),
        // Top-right (green) into the bottom-left.
        Sprite::new(
            quarter(2.0, 0.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(0.0, 64.0)),
        ),
        // Top-left (red) into the bottom-right.
        Sprite::new(
            quarter(0.0, 0.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(64.0, 64.0)),
        ),
    ]
}

#[test]
fn an_atlas_draws_each_sprite_from_the_part_of_the_sheet_it_named() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_atlas(
            &shuffled_quadrants(),
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    for (x, y, want, place) in [
        (32u32, 32u32, [255u8, 255, 0, 255], "top-left"),
        (96, 32, [0, 0, 255, 255], "top-right"),
        (32, 96, [0, 255, 0, 255], "bottom-left"),
        (96, 96, [255, 0, 0, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {place} sprite read the wrong part of the sheet"
        );
    }
}

#[test]
fn an_atlas_is_one_draw_however_many_sprites_it_holds() {
    // Four sprites, one draw. This was the whole reason the call existed when
    // it was written: a loop over `draw_vertices` was four draws then. Batches
    // merge adjacent draws that differ in nothing now, so that loop would also
    // come to one -- what the call still saves is building each sprite's quad
    // and its coordinates, which is where the axes get transposed.
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_atlas(
            &shuffled_quadrants(),
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");
    let recording = canvas.finish();
    let pass = recording.passes.first().expect("one pass");
    assert_eq!(
        pass.batch.draw_count(),
        1,
        "four sprites should be one draw, not one draw each"
    );
}

#[test]
fn an_atlas_sprite_travels_through_its_own_transform() {
    // A quarter turn about the sprite's own center, which is the case the
    // transform exists for: the sprite is square and its colors are not, so a
    // rotation is visible in where the halves land rather than only in the
    // outline.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    // The whole sheet, drawn at 128 by 128, turned a quarter turn about its
    // center. Clockwise on screen, since y runs downward.
    let placement = Affine2::from_translation(Vec2::splat(64.0))
        * Affine2::from_angle(std::f32::consts::FRAC_PI_2)
        * Affine2::from_translation(Vec2::splat(-64.0))
        * Affine2::from_scale(Vec2::splat(32.0));
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_atlas(
            &[Sprite::new(SourceRect::new(0.0, 0.0, 4.0, 4.0), placement)],
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Turned a quarter clockwise, the sheet's top-left corner is now at the
    // top-right of the target.
    for (x, y, want, place) in [
        (96u32, 32u32, [255u8, 0, 0, 255], "top-right"),
        (96, 96, [0, 255, 0, 255], "bottom-right"),
        (32, 96, [255, 255, 0, 255], "bottom-left"),
        (32, 32, [0, 0, 255, 255], "top-left"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {place} corner is not where a quarter turn puts it"
        );
    }
}

#[test]
fn an_atlas_needs_the_size_of_the_sheet_it_reads() {
    let mut canvas = Canvas::new(SIZE);
    assert!(
        canvas
            .draw_atlas(
                &shuffled_quadrants(),
                Extent2D::new(0, 4),
                &Paint::image(0, Rect::from_size(128.0, 128.0)),
            )
            .is_err(),
        "a sheet with no texels should be refused rather than divided by"
    );
}

#[test]
fn a_blend_filter_agrees_with_the_same_blend_done_by_the_blender() {
    // The check the whole derivation rests on. Every Porter-Duff mode is
    // affine in its destination once the source is a constant, which is why
    // these can be matrices at all -- and the way to know the matrices are
    // right is that the hardware, computing the same blend its own way, agrees.
    //
    // One side draws the material and then draws the constant over it with the
    // mode. The other draws the material once, with the mode compiled into a
    // filter. Two entirely different paths through the pipeline: fixed-function
    // blending against the framebuffer, and four multiply-adds in the fragment.
    let Some(mut ctx) = context() else { return };

    let material = Color::linear(0.6, 0.2, 0.1, 0.8);
    let constant = Color::linear(0.2, 0.7, 0.4, 0.5);
    let area = Rect::from_size(128.0, 128.0);

    for mode in [
        BlendMode::SrcOver,
        BlendMode::DstOver,
        BlendMode::SrcIn,
        BlendMode::DstIn,
        BlendMode::SrcOut,
        BlendMode::DstOut,
        BlendMode::SrcATop,
        BlendMode::DstATop,
        BlendMode::Xor,
        BlendMode::Plus,
        BlendMode::Modulate,
        BlendMode::Src,
        BlendMode::Dst,
        BlendMode::Clear,
    ] {
        // The blender's answer: the material, replaced into the target, then
        // the constant blended over it.
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
        canvas
            .draw_rect(
                area,
                &Paint::fill(material)
                    .with_blend(BlendMode::Src)
                    .with_anti_alias(false),
            )
            .expect("material");
        canvas
            .draw_rect(
                area,
                &Paint::fill(constant)
                    .with_blend(mode)
                    .with_anti_alias(false),
            )
            .expect("constant");
        let blended = render(&mut ctx, canvas);

        // The filter's answer: one draw, with the same blend compiled in.
        let filter = ColorFilter::blend(constant.to_array(), mode)
            .unwrap_or_else(|e| panic!("{mode:?} should be expressible as a filter: {e}"));
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
        canvas
            .draw_rect(
                area,
                &Paint::fill(material)
                    .with_color_filter(filter)
                    .with_blend(BlendMode::Src)
                    .with_anti_alias(false),
            )
            .expect("filtered");
        let filtered = render(&mut ctx, canvas);

        let (a, b) = (pixel(&blended, 64, 64), pixel(&filtered, 64, 64));
        for channel in 0..4 {
            assert!(
                (a[channel] as i32 - b[channel] as i32).abs() <= 2,
                "{mode:?} as a filter gives {b:?} where the blender gives {a:?}"
            );
        }
    }
}

#[test]
fn a_colour_matrix_recolours_a_gradient_which_no_tint_could() {
    // What the row in the parity table means by tinting anything rather than
    // only an image: the filter applies to the color the shader produced, so a
    // gradient is recolored along its whole length rather than at its stops.
    // The matrix swaps red and blue, so the two ends must trade places.
    let Some(mut ctx) = context() else { return };

    #[rustfmt::skip]
    let swap = ColorFilter::matrix([
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.0,
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);
    let stops = || {
        vec![
            GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
            GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
        ]
    };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(Vec2::new(8.0, 0.0), Vec2::new(120.0, 0.0), stops())
                .with_color_filter(swap)
                .with_anti_alias(false),
        )
        .expect("gradient");
    let pixels = render(&mut ctx, canvas);

    let left = pixel(&pixels, 4, 64);
    let right = pixel(&pixels, 124, 64);
    assert!(
        left[2] > 240 && left[0] < 16,
        "the red end should have become blue, got {left:?}"
    );
    assert!(
        right[0] > 240 && right[2] < 16,
        "the blue end should have become red, got {right:?}"
    );

    // And the middle is still a mixture rather than either end, which is what
    // says the filter ran per fragment and not on the stops.
    let middle = pixel(&pixels, 64, 64);
    assert!(
        middle[0] > 40 && middle[0] < 215 && middle[2] > 40 && middle[2] < 215,
        "the middle of the gradient should still be a mixture, got {middle:?}"
    );
}

#[test]
fn an_advanced_blend_mode_is_refused_as_a_filter_rather_than_approximated() {
    // These are piecewise or exchange components between channels, so no
    // matrix is equal to them. The paint's own blend mode is the hardware path
    // for exactly these, and the message says so.
    for mode in [
        BlendMode::Overlay,
        BlendMode::HardLight,
        BlendMode::Difference,
        BlendMode::Hue,
        BlendMode::Luminosity,
    ] {
        assert!(
            ColorFilter::blend([1.0, 1.0, 1.0, 1.0], mode).is_err(),
            "{mode:?} is not affine and must not become a matrix"
        );
    }
}

#[test]
fn a_colour_matrix_is_applied_to_straight_colour_not_premultiplied() {
    // The distinction only shows on a translucent color, which is why it needs
    // its own test: everything else here is opaque, and on an opaque color the
    // two forms are the same function.
    //
    // The matrix adds a half to green and leaves the rest alone. On straight
    // color that is a half of green at full strength, which premultiplies to a
    // quarter. Applied to the premultiplied color instead it would be a half
    // outright -- twice as much green, and the number this test names.
    let Some(mut ctx) = context() else { return };

    #[rustfmt::skip]
    let add_green = ColorFilter::matrix([
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.5,
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 0.5))
                .with_color_filter(add_green)
                .with_anti_alias(false),
        )
        .expect("filtered");
    let pixels = render(&mut ctx, canvas);

    // Half a unit of red and a quarter of green, over black.
    let got = pixel(&pixels, 64, 64);
    assert!(
        (got[0] as i32 - 128).abs() <= 2,
        "red should be halved by the alpha, got {got:?}"
    );
    assert!(
        (got[1] as i32 - 64).abs() <= 2,
        "green should be a quarter: half of a half. Twice that means the matrix \
         was applied to premultiplied color. Got {got:?}"
    );
}

#[test]
fn a_luminance_matrix_turns_every_colour_the_same_grey_it_weighs() {
    // The canonical color filter, and the one that pins the matrix's
    // orientation. Every row is the same set of weights, so the matrix is not
    // symmetric -- transposed, it would scale each channel by its own weight
    // and leave red red instead of making it grey. The two matrices this file
    // tests elsewhere are both symmetric and cannot tell the difference.
    let Some(mut ctx) = context() else { return };

    const R: f32 = 0.2126;
    const G: f32 = 0.7152;
    const B: f32 = 0.0722;
    #[rustfmt::skip]
    let grey = ColorFilter::matrix([
        R, G, B, 0.0, 0.0,
        R, G, B, 0.0, 0.0,
        R, G, B, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);

    for (name, color, want) in [
        ("red", Color::linear(1.0, 0.0, 0.0, 1.0), R),
        ("green", Color::linear(0.0, 1.0, 0.0, 1.0), G),
        ("blue", Color::linear(0.0, 0.0, 1.0, 1.0), B),
    ] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(color)
                    .with_color_filter(grey)
                    .with_anti_alias(false),
            )
            .expect("filtered");
        let pixels = render(&mut ctx, canvas);
        let got = pixel(&pixels, 64, 64);

        let expected = (want * 255.0).round() as i32;
        for channel in 0..3 {
            assert!(
                (got[channel] as i32 - expected).abs() <= 2,
                "{name} should weigh to {expected} in every channel, got {got:?}"
            );
        }
    }
}

#[test]
fn a_mesh_interpolates_the_colours_its_vertices_carry() {
    // What per-vertex color is for: a gradient across a triangle that no
    // gradient shader describes, because the three corners are independent.
    // Each corner must be its own color and the middle a mixture of all three,
    // which is what says the rasterizer interpolated rather than picking one.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let mesh = Vertices::colored(
        VertexMode::Triangles,
        vec![
            Vec2::new(64.0, 8.0),
            Vec2::new(120.0, 112.0),
            Vec2::new(8.0, 112.0),
        ],
        vec![
            Color::linear(1.0, 0.0, 0.0, 1.0),
            Color::linear(0.0, 1.0, 0.0, 1.0),
            Color::linear(0.0, 0.0, 1.0, 1.0),
        ],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    // Well inside each corner, where one color dominates.
    for (x, y, channel, corner) in [
        (64u32, 20u32, 0usize, "top"),
        (108, 104, 1, "bottom-right"),
        (20, 104, 2, "bottom-left"),
    ] {
        let got = pixel(&pixels, x, y);
        let others: Vec<u8> = (0..3).filter(|c| *c != channel).map(|c| got[c]).collect();
        assert!(
            got[channel] > 150 && others.iter().all(|v| *v < 110),
            "the {corner} corner should be dominated by its own color, got {got:?}"
        );
    }

    // The middle is all three at once, which no single vertex is.
    let middle = pixel(&pixels, 64, 80);
    assert!(
        (0..3).all(|c| middle[c] > 40 && middle[c] < 160),
        "the middle should mix all three corners, got {middle:?}"
    );
}

#[test]
fn a_vertex_colour_multiplies_the_paint_rather_than_replacing_it() {
    // The rule that makes a white paint leave the mesh's colors alone and any
    // other paint shade them. A half-strength red paint under a green vertex
    // color has to give neither red nor green but their product, which is
    // black -- so the test uses a paint that shares a channel with the color.
    let Some(mut ctx) = context() else { return };

    let corners = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(128.0, 0.0),
        Vec2::new(0.0, 128.0),
    ];
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let mesh = Vertices::colored(
        VertexMode::Triangles,
        corners,
        vec![Color::linear(1.0, 0.5, 0.0, 1.0); 3],
    )
    .expect("mesh");
    canvas
        .draw_vertices(
            &mesh,
            &Paint::fill(Color::linear(0.5, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    // (1.0, 0.5, 0.0) times (0.5, 1.0, 1.0) is (0.5, 0.5, 0.0).
    let got = pixel(&pixels, 20, 20);
    for (channel, want) in [(0usize, 128i32), (1, 128), (2, 0)] {
        assert!(
            (got[channel] as i32 - want).abs() <= 2,
            "channel {channel} should be the product of paint and vertex, got {got:?}"
        );
    }
}

#[test]
fn an_atlas_tints_each_sprite_on_its_own_in_one_draw() {
    // The reason a sprite carries a color rather than the batch carrying one:
    // a palette out of a single monochrome sheet is the whole idiom, and doing
    // it per batch would mean one draw per color, which is the thing the call
    // exists to avoid.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    // Opaque white throughout, so what comes out is the tint alone.
    ctx.write_image(&mut image, &[255u8; 4 * 4 * 4])
        .expect("upload");

    let whole = SourceRect::new(0.0, 0.0, 4.0, 4.0);
    let place =
        |x: f32| Affine2::from_scale_angle_translation(Vec2::splat(16.0), 0.0, Vec2::new(x, 0.0));
    let sprites = vec![
        Sprite::new(whole, place(0.0)).with_color(Color::linear(1.0, 0.0, 0.0, 1.0)),
        Sprite::new(whole, place(64.0)).with_color(Color::linear(0.0, 1.0, 0.0, 1.0)),
    ];

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_atlas(
            &sprites,
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");
    let recording = canvas.finish();
    assert_eq!(
        recording.passes[0].batch.draw_count(),
        1,
        "tinting per sprite must not split the batch"
    );

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &recording, &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    assert_eq!(pixel(&pixels, 32, 32), [255, 0, 0, 255], "the first sprite");
    assert_eq!(
        pixel(&pixels, 96, 32),
        [0, 255, 0, 255],
        "the second sprite"
    );
}

#[test]
fn a_vertex_colour_is_interpolated_premultiplied_across_a_fading_edge() {
    // The distinction only appears where alpha varies between vertices, which
    // is why the other color tests cannot see it: on constant alpha the two
    // forms are the same numbers.
    //
    // A quad red at one edge and transparent at the other. Halfway along, a
    // premultiplied interpolation gives half a unit of red at half alpha,
    // which over black is half. Interpolating straight color and using it as
    // though it were premultiplied gives a full unit of red at half alpha --
    // a color brighter than the alpha it is multiplied by, which is not a
    // color, and which over black is twice as bright.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let mesh = Vertices::full(
        VertexMode::Triangles,
        vec![
            Vec2::new(8.0, 32.0),
            Vec2::new(120.0, 32.0),
            Vec2::new(120.0, 96.0),
            Vec2::new(8.0, 96.0),
        ],
        Vec::new(),
        vec![
            Color::linear(1.0, 0.0, 0.0, 1.0),
            Color::linear(1.0, 0.0, 0.0, 0.0),
            Color::linear(1.0, 0.0, 0.0, 0.0),
            Color::linear(1.0, 0.0, 0.0, 1.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh");
    canvas
        .draw_vertices(
            &mesh,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    let middle = pixel(&pixels, 64, 64);
    assert!(
        (middle[0] as i32 - 128).abs() <= 6,
        "halfway along the fade should be half strength; twice that means the \
         color was interpolated straight and then used as premultiplied. Got {middle:?}"
    );
}

#[test]
fn nearest_sampling_reads_one_texel_where_linear_blends_two() {
    // The four-by-four fixture magnified thirty-two times, so each texel covers
    // a wide band and the boundary between two of them is the place the two
    // modes disagree. Linear blends across it; nearest steps.
    //
    // Sampled just either side of the midpoint, which is where the boundary
    // lands: linear must give something between the two colors there, and
    // nearest must give one of them exactly.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let draw = |ctx: &mut Context, sampling: Sampling| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::image(0, Rect::from_size(128.0, 128.0))
                    .with_sampling(sampling)
                    .with_anti_alias(false),
            )
            .expect("image");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let linear = draw(&mut ctx, Sampling::Linear);
    let nearest = draw(&mut ctx, Sampling::Nearest);
    ctx.destroy_image(image);

    // The seam between the red and green quadrants runs down the middle. Two
    // texels meet there, so a linear read a pixel to either side is a mixture
    // of both.
    for x in [62u32, 66] {
        let got = pixel(&linear, x, 32);
        assert!(
            got[0] > 20 && got[1] > 20,
            "linear at ({x}, 32) should mix the two quadrants, got {got:?}"
        );
    }

    // Nearest reads whichever texel the coordinate falls in, so the same two
    // places are the two colors outright.
    assert_eq!(
        pixel(&nearest, 62, 32),
        [255, 0, 0, 255],
        "nearest left of the seam should be the red texel alone"
    );
    assert_eq!(
        pixel(&nearest, 66, 32),
        [0, 255, 0, 255],
        "nearest right of the seam should be the green texel alone"
    );

    // And well inside a quadrant the two agree, which says nearest moved the
    // coordinate rather than changing what it addressed.
    for (x, y) in [(16u32, 16u32), (112, 112)] {
        assert_eq!(
            pixel(&linear, x, y),
            pixel(&nearest, x, y),
            "the two modes should agree away from a boundary at ({x}, {y})"
        );
    }
}

#[test]
fn an_image_filter_blur_of_a_solid_agrees_with_the_mask_blur_of_the_same_shape() {
    // The two are defined differently and only coincide here. A mask blur
    // blurs coverage and then fills; an image filter fills and then blurs the
    // result. Those are the same picture exactly when the fill does not vary,
    // which is why a mask blur takes a solid color and this takes anything --
    // and on a solid color they must agree, or one of them is wrong.
    let Some(mut ctx) = context() else { return };

    let shape = || {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(32.0, 40.0))
            .line_to(Vec2::new(96.0, 40.0))
            .line_to(Vec2::new(64.0, 96.0))
            .close();
        b.build()
    };
    let color = Color::linear(0.9, 0.3, 0.2, 1.0);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.05, 0.05, 0.08, 1.0));
    canvas
        .draw_path(&shape(), &Paint::fill(color).with_mask_blur(6.0))
        .expect("mask blur");
    let masked = render(&mut ctx, canvas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.05, 0.05, 0.08, 1.0));
    canvas
        .draw_path(
            &shape(),
            &Paint::fill(color).with_image_filter(ImageFilter::Blur { sigma: 6.0 }),
        )
        .expect("image filter");
    let filtered = render(&mut ctx, canvas);

    let mut worst = 0i32;
    for (a, b) in masked.chunks_exact(4).zip(filtered.chunks_exact(4)) {
        for channel in 0..4 {
            worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
        }
    }
    assert!(
        worst <= 2,
        "blurring coverage and blurring the result should agree on a solid \
         colour; they differ by {worst}"
    );
}

#[test]
fn an_image_filter_blurs_a_gradient_that_a_mask_blur_refuses() {
    // What the filter is for. The same call on the same paint is refused as a
    // mask blur, because blurring coverage and then filling with something
    // that varies is a different picture from blurring the result -- and
    // drawing one where the caller asked for the other is the substitution
    // this renderer declines to make.
    let Some(mut ctx) = context() else { return };

    let gradient = || {
        Paint::linear_gradient(
            Vec2::new(32.0, 0.0),
            Vec2::new(96.0, 0.0),
            vec![
                GradientStop::new(Color::linear(1.0, 0.2, 0.0, 1.0), 0.0),
                GradientStop::new(Color::linear(0.0, 0.4, 1.0, 1.0), 1.0),
            ],
        )
    };
    let area = Rect::new(32.0, 40.0, 96.0, 88.0);

    let mut canvas = Canvas::new(SIZE);
    assert!(
        canvas
            .draw_rect(area, &gradient().with_mask_blur(6.0))
            .is_err(),
        "a mask blur of a gradient should still be refused"
    );

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            area,
            &gradient().with_image_filter(ImageFilter::Blur { sigma: 6.0 }),
        )
        .expect("image filter");
    let pixels = render(&mut ctx, canvas);

    // Soft at the edge: a pixel just outside the rectangle has colour, and one
    // well outside has none.
    let just_outside = pixel(&pixels, 64, 36);
    let far_outside = pixel(&pixels, 64, 8);
    assert!(
        just_outside.iter().take(3).any(|v| *v > 20),
        "the blur should reach past the shape, got {just_outside:?}"
    );
    assert_eq!(
        far_outside,
        [0, 0, 0, 255],
        "the blur should not reach the whole target"
    );

    // And it is still a gradient: the two ends differ in the way the stops say.
    let left = pixel(&pixels, 40, 64);
    let right = pixel(&pixels, 88, 64);
    assert!(
        left[0] > right[0] && right[2] > left[2],
        "the gradient should survive the blur: left {left:?}, right {right:?}"
    );
}

#[test]
fn a_filtered_stroke_keeps_the_half_of_itself_that_lies_outside_the_path() {
    // The layer a filter draws into is bounded, and a stroke reaches half its
    // width past the path the bounds come from. That widening is easy to miss
    // and hard to catch: opening a bounded layer with a blur already widens
    // the region by three deviations, which covers any stroke narrower than
    // that. So this uses a wide stroke and a small blur, where the two
    // widenings are nothing like each other.
    //
    // A circle of radius twenty stroked forty wide covers everything within
    // forty of its centre. Without the stroke's own reach the layer would end
    // about twenty-three out, and the outer half of the band would be clipped
    // away -- which looks like a thinner stroke rather than like a bug.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(64.0, 64.0),
            20.0,
            &Paint::stroke(Color::linear(1.0, 1.0, 1.0, 1.0), 40.0).with_mask_blur(1.0),
        )
        .expect("stroke");
    let pixels = render(&mut ctx, canvas);

    // Well inside the band's outer half, which only exists if the bounds took
    // the stroke into account.
    for (x, y) in [(64u32, 28u32), (64, 100), (28, 64), (100, 64)] {
        let got = pixel(&pixels, x, y);
        assert!(
            got[0] > 200,
            "({x}, {y}) is inside the stroke and should be painted, got {got:?}"
        );
    }
}

/// A shape and the three places a blur style is decided: well inside it, on
/// its edge, and outside but within the blur's reach.
fn mask_blur_probe(ctx: &mut Context, style: MaskBlurStyle) -> ([u8; 4], [u8; 4], [u8; 4]) {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(64.0, 64.0),
            34.0,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                .with_mask_blur(6.0)
                .with_mask_blur_style(style),
        )
        .expect("mask blur");
    let pixels = render(ctx, canvas);
    (
        pixel(&pixels, 64, 64),
        pixel(&pixels, 64, 30),
        pixel(&pixels, 64, 18),
    )
}

#[test]
fn each_mask_blur_style_keeps_the_part_of_the_blur_it_names() {
    // The four styles are four combinations of two things: the shape's own
    // coverage, and that coverage blurred. Naming them is only useful if each
    // actually discards what it says it discards, which is what these three
    // probes are for -- inside the shape, on its edge, and outside it but
    // within the blur's reach.
    let Some(mut ctx) = context() else { return };

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Normal);
    assert!(
        inside[0] > 240,
        "normal should be solid well inside: {inside:?}"
    );
    assert!(
        edge[0] > 40 && edge[0] < 220,
        "normal should be soft at the edge: {edge:?}"
    );
    assert!(outside[0] > 4, "normal should reach outside: {outside:?}");

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Solid);
    assert!(inside[0] > 240, "solid should be solid inside: {inside:?}");
    assert!(
        edge[0] > 240,
        "solid keeps the shape at full strength, so its own edge is hard: {edge:?}"
    );
    assert!(
        outside[0] > 4,
        "solid should still blur outside: {outside:?}"
    );

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Outer);
    assert_eq!(
        inside,
        [0, 0, 0, 255],
        "outer draws nothing inside the shape"
    );
    assert!(outside[0] > 4, "outer is the blur outside: {outside:?}");
    assert!(
        edge[0] < 240,
        "the shape is taken out of the blur, so its edge is not full strength: {edge:?}"
    );

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Inner);
    assert_eq!(
        outside,
        [0, 0, 0, 255],
        "inner draws nothing outside the shape"
    );
    assert!(
        inside[0] > 240,
        "inner is solid away from the edge: {inside:?}"
    );
    assert!(
        edge[0] > 4 && edge[0] < 240,
        "inner fades toward the shape's own edge: {edge:?}"
    );
}

#[test]
fn the_two_halves_of_a_blur_add_up_to_the_whole_of_it() {
    // Outer and inner partition the normal style between them: one keeps the
    // blurred coverage where the shape is not, the other where it is, and
    // neither invents anything. So at every pixel the two have to add back up
    // to the whole -- a stronger statement than any of the three makes alone,
    // and the one that catches a style keeping slightly too much.
    //
    // Drawn separately and added here rather than drawn on top of each other,
    // because a paint's blend mode applies to the draw *inside* the layer a
    // mask blur opens, not to the layer's composite. Two styles drawn in
    // sequence therefore composite with source-over, which is not addition
    // wherever both are non-zero -- which is exactly the edge this is about.
    let Some(mut ctx) = context() else { return };

    let draw = |ctx: &mut Context, style: MaskBlurStyle| {
        let mut canvas = Canvas::new(SIZE);
        // Black, so what comes back at each pixel is the contribution itself:
        // source-over onto zero leaves the premultiplied colour alone.
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                34.0,
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                    .with_mask_blur(6.0)
                    .with_mask_blur_style(style),
            )
            .expect("mask blur");
        render(ctx, canvas)
    };

    let whole = draw(&mut ctx, MaskBlurStyle::Normal);
    let inner = draw(&mut ctx, MaskBlurStyle::Inner);
    let outer = draw(&mut ctx, MaskBlurStyle::Outer);

    let mut worst = 0i32;
    let mut worst_at = 0usize;
    for (i, ((w, a), b)) in whole
        .chunks_exact(4)
        .zip(inner.chunks_exact(4))
        .zip(outer.chunks_exact(4))
        .enumerate()
    {
        // The red channel alone: the fill is white, so all three carry the
        // same number, and the fourth is the target's own opaque alpha.
        let delta = (w[0] as i32 - (a[0] as i32 + b[0] as i32)).abs();
        if delta > worst {
            worst = delta;
            worst_at = i;
        }
    }
    assert!(
        worst <= 4,
        "the inner and outer halves should reconstruct the whole blur; they \
         differ by {worst} at pixel ({}, {})",
        worst_at % 128,
        worst_at / 128
    );
}

/// A card, and a shadow beneath it at the given elevation.
fn shadow_probe(ctx: &mut Context, elevation: f32, transparent: bool, draw_card: bool) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.9, 0.9, 0.92, 1.0));
    let card = {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(40.0, 40.0))
            .line_to(Vec2::new(88.0, 40.0))
            .line_to(Vec2::new(88.0, 80.0))
            .line_to(Vec2::new(40.0, 80.0))
            .close();
        b.build()
    };
    canvas
        .draw_shadow(
            &card,
            Color::linear(0.0, 0.0, 0.0, 1.0),
            elevation,
            transparent,
        )
        .expect("shadow");
    if draw_card {
        canvas
            .draw_path(&card, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
            .expect("card");
    }
    render(ctx, canvas)
}

#[test]
fn a_shadow_falls_below_what_casts_it_and_widens_with_elevation() {
    // The whole content of an elevation: an object further from the surface
    // throws its shadow further and softer. Both have to move together, or the
    // number means nothing beyond "more blur".
    let Some(mut ctx) = context() else { return };

    let low = shadow_probe(&mut ctx, 3.0, false, true);
    let high = shadow_probe(&mut ctx, 12.0, false, true);

    // Just below the card, where a shadow lands and where the higher one must
    // be darker because it reaches further.
    let below_low = pixel(&low, 64, 88)[0] as i32;
    let below_high = pixel(&high, 64, 88)[0] as i32;
    assert!(
        below_high < below_low,
        "raising the object should darken the ground below it: {below_low} then {below_high}"
    );

    // Above the card there is no light behind the object, so nothing falls
    // there at either elevation.
    let above_low = pixel(&low, 64, 30)[0] as i32;
    let above_high = pixel(&high, 64, 30)[0] as i32;
    assert!(
        above_low > 220 && above_high > 220,
        "the light is above, so nothing should fall above the object: {above_low}, {above_high}"
    );

    // Beside the card, where the offset does not reach and only the blur can.
    // Without this the test passes on a shadow whose softness ignores the
    // elevation entirely, since moving it down alone darkens the ground below.
    let beside_low = pixel(&low, 30, 60)[0] as i32;
    let beside_high = pixel(&high, 30, 60)[0] as i32;
    assert!(
        beside_high < beside_low - 8,
        "raising the object should spread its shadow sideways too: {beside_low} then {beside_high}"
    );
}

#[test]
fn an_object_resting_on_the_surface_casts_no_shadow() {
    let Some(mut ctx) = context() else { return };
    let flat = shadow_probe(&mut ctx, 0.0, false, false);
    let first = &flat[0..4];
    assert!(
        flat.chunks_exact(4).all(|texel| texel == first),
        "an elevation of zero should leave the ground untouched"
    );
}

#[test]
fn an_opaque_occluder_is_not_given_a_shadow_it_would_hide() {
    // The part of a shadow its caster will cover is spent, and this is the
    // flag saying whether it will. What covers it is the object where it
    // actually sits rather than the shadow's own outline -- the same shape in
    // two places -- so removing the wrong one leaves a crescent showing above
    // the object and takes one out below it.
    let Some(mut ctx) = context() else { return };

    let opaque = shadow_probe(&mut ctx, 10.0, false, false);
    let transparent = shadow_probe(&mut ctx, 10.0, true, false);

    // Well inside the card, and well inside the offset shadow too, so the
    // shadow is at full strength there rather than on its own soft edge.
    let under_opaque = pixel(&opaque, 64, 70)[0] as i32;
    let under_transparent = pixel(&transparent, 64, 70)[0] as i32;
    // The ground itself, taken from a corner nothing reaches. Compared against
    // rather than a threshold, because "nothing was drawn" and "something
    // brighter than the threshold was drawn" are different claims and only the
    // first one is this test's -- punching the shadow out with the wrong blend
    // paints white there and passes any upper bound.
    let ground = pixel(&opaque, 4, 4)[0] as i32;
    assert!(
        (under_opaque - ground).abs() <= 2,
        "an opaque occluder should leave the ground as it was: {under_opaque} against {ground}"
    );
    assert!(
        under_transparent < 190,
        "a transparent one should: {under_transparent}"
    );

    // And below the card, past where it sits, both must show the shadow --
    // the punch takes out the object's own area and nothing more.
    for (name, pixels) in [("opaque", &opaque), ("transparent", &transparent)] {
        let below = pixel(pixels, 64, 86)[0] as i32;
        assert!(
            below < 200,
            "{name}: the shadow past the object should survive: {below}"
        );
    }
}

#[test]
fn a_point_is_drawn_as_the_cap_it_would_have_had() {
    // A point is a segment of no length, so the cap is the whole shape. Round
    // gives a dot, square gives a square, and butt -- which extends a segment
    // by nothing -- gives nothing. That last one is not a special case; it is
    // the only reading that stays consistent with how a segment is drawn.
    let Some(mut ctx) = context() else { return };

    let draw = |ctx: &mut Context, cap: LineCap| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_points(
                PointMode::Points,
                &[Vec2::new(64.0, 64.0)],
                &Paint {
                    style: Style::Stroke(StrokeStyle {
                        cap,
                        ..StrokeStyle::new(40.0)
                    }),
                    ..Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false)
                },
            )
            .expect("points");
        render(ctx, canvas)
    };

    let round = draw(&mut ctx, LineCap::Round);
    let square = draw(&mut ctx, LineCap::Square);
    let butt = draw(&mut ctx, LineCap::Butt);

    // The corner of the square the point spans: inside a square cap, outside
    // a round one.
    let corner = (64 + 18, 64 + 18);
    assert!(
        pixel(&square, corner.0, corner.1)[0] > 240,
        "a square cap should fill its corners"
    );
    assert_eq!(
        pixel(&round, corner.0, corner.1),
        [0, 0, 0, 255],
        "a round cap should not reach its corners"
    );
    assert!(
        pixel(&round, 64, 64)[0] > 240 && pixel(&square, 64, 64)[0] > 240,
        "both should cover the point itself"
    );

    let first = &butt[0..4];
    assert!(
        butt.chunks_exact(4).all(|texel| texel == first),
        "a butt cap adds nothing to a segment of no length, so nothing is drawn"
    );
}

#[test]
fn a_double_rounded_rect_is_a_ring_rather_than_two_shapes() {
    // Two contours wound the same way fill solid under the nonzero rule and
    // hollow under even-odd, and a border wants the second. So the test is
    // that the middle is untouched -- not that something was drawn, which
    // both rules satisfy.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_drrect(
            Rect::new(16.0, 16.0, 112.0, 112.0),
            18.0,
            Rect::new(40.0, 40.0, 88.0, 88.0),
            10.0,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("ring");
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 64, 64),
        [0, 0, 0, 255],
        "the inner rectangle should be a hole"
    );
    assert!(
        pixel(&pixels, 64, 28)[0] > 240,
        "the band between them should be filled"
    );
    assert_eq!(
        pixel(&pixels, 4, 4),
        [0, 0, 0, 255],
        "and nothing outside the outer one"
    );
}

#[test]
fn the_clip_bounds_narrow_with_every_kind_of_clip() {
    // What a caller culls against. A scissor and a stencil clip narrow what
    // may be drawn by quite different means -- one is recorder state, the
    // other a buffer on the device -- and a caller asking what is still
    // reachable wants both accounted for.
    let mut canvas = Canvas::new(SIZE);
    let whole = canvas.destination_clip_bounds();
    assert_eq!(
        (whole.left, whole.top, whole.right, whole.bottom),
        (0.0, 0.0, 128.0, 128.0),
        "an unclipped canvas may reach all of its target"
    );

    canvas.save();
    canvas.clip_rect(Rect::new(16.0, 16.0, 96.0, 96.0)).unwrap();
    let after_rect = canvas.destination_clip_bounds();
    assert_eq!(
        (
            after_rect.left,
            after_rect.top,
            after_rect.right,
            after_rect.bottom
        ),
        (16.0, 16.0, 96.0, 96.0)
    );

    // A path clip goes to the stencil and leaves the scissor alone, so the
    // tracked rectangle is the only thing that sees it.
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(32.0, 32.0))
        .line_to(Vec2::new(64.0, 32.0))
        .line_to(Vec2::new(64.0, 64.0))
        .close();
    canvas.clip_path(&b.build()).unwrap();
    let after_path = canvas.destination_clip_bounds();
    assert_eq!(
        (
            after_path.left,
            after_path.top,
            after_path.right,
            after_path.bottom
        ),
        (32.0, 32.0, 64.0, 64.0),
        "a path clip narrows what is reachable even though the scissor is unchanged"
    );

    canvas.restore();
    let restored = canvas.destination_clip_bounds();
    assert_eq!(
        (restored.left, restored.top, restored.right, restored.bottom),
        (0.0, 0.0, 128.0, 128.0),
        "restoring gives back what the save was holding"
    );
}

#[test]
fn the_local_clip_bounds_are_stated_in_the_callers_own_coordinates() {
    // The same region seen from where the caller is drawing. Under a
    // translation and a scale it is exact; under a rotation it is the box
    // around the rotated box, which is larger than the clip and deliberately
    // so -- too large costs a draw that turns out to be invisible, too small
    // loses a shape.
    let mut canvas = Canvas::new(SIZE);
    canvas.clip_rect(Rect::new(32.0, 32.0, 96.0, 96.0)).unwrap();

    canvas.save();
    canvas.translate(32.0, 32.0);
    let local = canvas.local_clip_bounds();
    assert_eq!(
        (local.left, local.top, local.right, local.bottom),
        (0.0, 0.0, 64.0, 64.0),
        "a translation moves the origin the bounds are stated from"
    );
    canvas.restore();

    canvas.save();
    canvas.scale(2.0, 2.0);
    let local = canvas.local_clip_bounds();
    assert_eq!(
        (local.left, local.top, local.right, local.bottom),
        (16.0, 16.0, 48.0, 48.0),
        "a scale changes what a unit is"
    );
    canvas.restore();

    canvas.save();
    canvas.rotate(std::f32::consts::FRAC_PI_4);
    let local = canvas.local_clip_bounds();
    // The box around a square turned an eighth of a turn is wider than the
    // square by a factor of root two, and must contain it.
    assert!(
        local.right - local.left > 64.0 * 1.4,
        "a rotation should give a conservative box, got {}",
        local.right - local.left
    );
    canvas.restore();
}

#[test]
fn a_collapsed_transform_leaves_nothing_reachable() {
    // A scale of zero folds the plane to a line and nothing drawn through it
    // covers anything. Reporting an enormous rectangle -- which inverting a
    // singular matrix invites -- would tell a caller to draw everything.
    let mut canvas = Canvas::new(SIZE);
    canvas.scale(0.0, 1.0);
    let local = canvas.local_clip_bounds();
    assert_eq!(
        (local.right - local.left, local.bottom - local.top),
        (0.0, 0.0),
        "a transform that cannot be inverted reaches nothing"
    );
}
