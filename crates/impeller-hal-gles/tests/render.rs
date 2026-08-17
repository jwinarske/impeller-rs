//! Offscreen rendering on the GLES backend.
//!
//! The assertions here are deliberately the same shape as the Vulkan ones —
//! exact pixel positions, exact channel values, computed blend results — so a
//! divergence between the backends shows up as a specific failing property
//! rather than as two suites that merely happen to pass.

use impeller_hal::{Batch, BlendMode, Extent2D, PassDescriptor, PixelFormat, TextureDescriptor};
use impeller_hal_gles::{DisplayTarget, GlesContext, GlesTexture};

const SIZE: u32 = 32;
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn context() -> Option<GlesContext> {
    match GlesContext::new(DisplayTarget::Surfaceless) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable GLES context ({e})");
            None
        }
    }
}

fn target(ctx: &mut GlesContext) -> GlesTexture {
    ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(SIZE, SIZE),
        PixelFormat::Rgba8Unorm,
    ))
    .expect("texture")
}

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A quad covering a horizontal band of clip space.
fn band(y0: f32, y1: f32) -> [[f32; 2]; 4] {
    [[-1.0, y0], [1.0, y0], [1.0, y1], [-1.0, y1]]
}

fn render(ctx: &mut GlesContext, batch: &Batch) -> Vec<u8> {
    let mut tex = target(ctx);
    ctx.submit_batch(&mut tex, batch, PassDescriptor::clear(BLACK))
        .expect("submit");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    pixels
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_full_quad_covers_the_target_in_the_paint_colour() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, [1.0, 0.0, 0.0, 1.0], BlendMode::Src)
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    assert!(
        pixels.chunks_exact(4).all(|p| p == [255, 0, 0, 255]),
        "expected a solid red target"
    );
}

#[test]
fn each_channel_of_the_paint_arrives_independently() {
    let Some(mut ctx) = context() else { return };
    // The paint reaches the shader through a lowered uniform rather than a push
    // constant, so this checks the lowering as much as the draw.
    for (color, expected) in [
        ([1.0, 0.0, 0.0, 1.0], [255, 0, 0, 255]),
        ([0.0, 1.0, 0.0, 1.0], [0, 255, 0, 255]),
        ([0.0, 0.0, 1.0, 1.0], [0, 0, 255, 255]),
    ] {
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, color, BlendMode::Src)
            .expect("push");
        let pixels = render(&mut ctx, &batch);
        assert_eq!(pixel(&pixels, 4, 4), expected, "for {color:?}");
    }
}

#[test]
fn clip_space_follows_the_wgsl_convention_with_y_up() {
    let Some(mut ctx) = context() else { return };
    // Clip Y in [-1, 0] is the lower half under the WGSL convention, so it must
    // land at the bottom of the image, which is exactly what the Vulkan backend
    // asserts. Framebuffer origins differ between the two APIs and the shader
    // translator cancels the difference, so no correction is applied on
    // readback; adding one would mirror the image, which this catches.
    let mut batch = Batch::new();
    batch
        .push(
            &band(-1.0, 0.0),
            &QUAD,
            [1.0, 1.0, 1.0, 1.0],
            BlendMode::Src,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    assert_eq!(pixel(&pixels, SIZE / 2, SIZE - 2)[0], 255, "bottom covered");
    assert_eq!(pixel(&pixels, SIZE / 2, 2)[0], 0, "top untouched");
}

#[test]
fn source_over_blends_against_what_is_already_there() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, [0.0, 0.0, 1.0, 1.0], BlendMode::Src)
        .expect("push");
    batch
        .push(&FULL, &QUAD, [1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver)
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    // The same arithmetic the Vulkan backend is held to: premultiplied source
    // over opaque blue gives half red, half blue, opaque.
    let got = pixel(&pixels, SIZE / 2, SIZE / 2);
    for (channel, want) in got.iter().zip([128u8, 0, 128, 255]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 1,
            "got {got:?}, expected about [128, 0, 128, 255]"
        );
    }
}

#[test]
fn draws_within_a_batch_keep_their_order() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, [1.0, 0.0, 0.0, 1.0], BlendMode::Src)
        .expect("push");
    batch
        .push(
            &band(-1.0, 0.0),
            &QUAD,
            [0.0, 1.0, 0.0, 1.0],
            BlendMode::Src,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    // Second draw wins where they overlap, first survives where they do not.
    assert_eq!(pixel(&pixels, SIZE / 2, SIZE - 2), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, SIZE / 2, 2), [255, 0, 0, 255]);
}

#[test]
fn index_offsets_address_each_draw_correctly() {
    let Some(mut ctx) = context() else { return };
    // Every draw after the first is offset into shared buffers, and GL takes
    // that offset in bytes rather than in indices. Getting the unit wrong draws
    // the first shape repeatedly, which two differently placed bands catch.
    let mut batch = Batch::new();
    batch
        .push(
            &band(-1.0, -0.5),
            &QUAD,
            [1.0, 0.0, 0.0, 1.0],
            BlendMode::Src,
        )
        .expect("push");
    batch
        .push(&band(0.5, 1.0), &QUAD, [0.0, 1.0, 0.0, 1.0], BlendMode::Src)
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    assert_eq!(
        pixel(&pixels, SIZE / 2, SIZE - 2),
        [255, 0, 0, 255],
        "first"
    );
    assert_eq!(pixel(&pixels, SIZE / 2, 1), [0, 255, 0, 255], "second");
    // And the middle stays as cleared, or the two draws overlapped.
    assert_eq!(pixel(&pixels, SIZE / 2, SIZE / 2), [0, 0, 0, 255], "middle");
}

#[test]
fn an_empty_batch_still_clears() {
    let Some(mut ctx) = context() else { return };
    let batch = Batch::new();
    let mut tex = target(&mut ctx);
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 1.0, 1.0]),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    assert_eq!(pixel(&pixels, 0, 0), [0, 0, 255, 255]);
}

#[test]
fn a_multisampled_pass_is_refused_rather_than_rendered_aliased() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, [1.0; 4], BlendMode::Src)
        .expect("push");
    let mut tex = target(&mut ctx);
    // Reporting success while producing aliased output would make the corpus
    // silently compare an antialiased image against an aliased one.
    let result = ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear(BLACK).with_samples(4),
    );
    assert!(result.is_err());
    ctx.destroy_texture(tex);
}

#[test]
fn textures_can_be_created_and_destroyed_repeatedly() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, [0.0, 1.0, 1.0, 1.0], BlendMode::Src)
        .expect("push");
    for i in 0..16 {
        let pixels = render(&mut ctx, &batch);
        assert_eq!(pixel(&pixels, 0, 0), [0, 255, 255, 255], "iteration {i}");
    }
}
