//! Texture upload and sampling, across both backends.
//!
//! The image paint is the first thing here that reads a resource rather than
//! only writing one, and the two backends bind it in quite different ways — a
//! descriptor set against a separate image and sampler on one, a texture unit
//! and a sampler uniform on the other. What that risks is a picture that is
//! plausible on each and different between them, so most of these compare the
//! two rather than only checking each against an expectation.

use impeller_hal::{
    Batch, BlendMode, Extent2D, Hal, HalContext, Material, PassDescriptor, PixelFormat,
    TextureDescriptor, TileMode,
};
use impeller_hal_gles::Validated as GlesValidated;
use impeller_hal_gles::{DisplayTarget, GlesHal};
use impeller_hal_vulkan::Validated;
use impeller_hal_vulkan::{DevicePreference, VulkanHal};

const SIZE: Extent2D = Extent2D {
    width: 32,
    height: 32,
};
const SOURCE: Extent2D = Extent2D {
    width: 4,
    height: 4,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A four-by-four image with a different color in each corner.
///
/// Asymmetric in both axes and in all four quadrants, so a flip, a transpose,
/// or a mirrored sampling coordinate each produce a distinguishable result.
/// Fully opaque, so a comparison is about the mapping rather than about alpha.
fn source_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; (SOURCE.area() * 4) as usize];
    for y in 0..SOURCE.height {
        for x in 0..SOURCE.width {
            let i = ((y * SOURCE.width + x) * 4) as usize;
            let left = x < SOURCE.width / 2;
            let top = y < SOURCE.height / 2;
            let color: [u8; 4] = match (left, top) {
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

/// Map the whole target to the whole image.
///
/// Clip space runs from minus one to one with Y upward, and texture
/// coordinates from zero to one with V downward, so the V axis is negated. The
/// origin is the top-left corner of the target in clip space, which is
/// `(-1, 1)`.
fn full_target_mapping() -> Material {
    Material::Image {
        origin: [-1.0, 1.0],
        to_local: [0.5, 0.0, 0.0, -0.5],
        slot: 0,
        alpha: 1.0,
        tile: TileMode::Clamp,
    }
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Upload the source image, then draw it over the whole target.
fn render<H: Hal>(ctx: &mut H::Context, material: Material) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    let mut source = ctx
        .create_texture(&TextureDescriptor::offscreen(
            SOURCE,
            PixelFormat::Rgba8Unorm,
        ))
        .expect("source texture");
    ctx.write_texture(&mut source, &source_pixels())
        .expect("upload");

    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, material, BlendMode::Src)
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&source],
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(source);
    pixels
}

#[test]
fn an_upload_survives_a_round_trip_through_readback() {
    // The inverse of readback, in the same layout, so this is the identity.
    // Everything below rests on it: a sampled image that came back wrong would
    // otherwise be as likely an upload fault as a sampling one.
    let want = source_pixels();
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want, "vulkan changed the pixels in transit");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want, "gles changed the pixels in transit");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

/// Check the four quadrants land where the source put them.
fn assert_quadrants(pixels: &[u8], backend: &str) {
    // Sampled well inside each quadrant, so a filter kernel at the boundary
    // does not blend two of them together.
    for (x, y, want, corner) in [
        (4u32, 4u32, [255u8, 0, 0, 255], "top-left"),
        (27, 4, [0, 255, 0, 255], "top-right"),
        (4, 27, [0, 0, 255, 255], "bottom-left"),
        (27, 27, [255, 255, 0, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(pixels, x, y),
            want,
            "{backend}: the {corner} quadrant sampled the wrong part of the image"
        );
    }
}

#[test]
fn an_image_lands_the_right_way_up_on_both_backends() {
    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let pixels = render::<VulkanHal>(&mut ctx, full_target_mapping());
        assert_quadrants(&pixels, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let pixels = render::<GlesHal>(&mut ctx, full_target_mapping());
        assert_quadrants(&pixels, "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn the_backends_agree_on_a_sampled_image() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };
    let a = render::<VulkanHal>(&mut vulkan, full_target_mapping());
    let b = render::<GlesHal>(&mut gles, full_target_mapping());

    // One unit, because a filtered sample is arithmetic and the two need not
    // round it identically. The quadrant centers above are exact regardless,
    // being far from any boundary the filter would blend across.
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "the backends differ by up to {worst}");
}

#[test]
fn alpha_scales_the_sampled_color() {
    let mut ran = 0;
    for backend in ["vulkan", "gles"] {
        let mut material = full_target_mapping();
        if let Material::Image { alpha, .. } = &mut material {
            *alpha = 0.5;
        }
        let pixels = match backend {
            "vulkan" => match Validated::new(DevicePreference::Auto) {
                Ok(mut ctx) => render::<VulkanHal>(&mut ctx, material),
                Err(_) => continue,
            },
            _ => match GlesValidated::new(DisplayTarget::Surfaceless) {
                Ok(mut ctx) => render::<GlesHal>(&mut ctx, material),
                Err(_) => continue,
            },
        };
        // Premultiplied on the way out, so a half-alpha red is half red and
        // half alpha rather than full red with a reduced alpha.
        let got = pixel(&pixels, 4, 4);
        for (channel, want) in got.iter().zip([128u8, 0, 0, 128]) {
            assert!(
                (*channel as i32 - want as i32).abs() <= 1,
                "{backend}: half-alpha red came back {got:?}"
            );
        }
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn the_tile_modes_differ_outside_the_image() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    // Map the image to the top-left quarter of the target, so three quarters of
    // it lie outside and the modes have somewhere to disagree.
    let quarter = |tile| Material::Image {
        origin: [-1.0, 1.0],
        to_local: [1.0, 0.0, 0.0, -1.0],
        slot: 0,
        alpha: 1.0,
        tile,
    };

    let clamp = render::<VulkanHal>(&mut ctx, quarter(TileMode::Clamp));
    let repeat = render::<VulkanHal>(&mut ctx, quarter(TileMode::Repeat));
    let decal = render::<VulkanHal>(&mut ctx, quarter(TileMode::Decal));

    // Inside the image, all three agree: the modes describe what happens
    // outside it, and a mode that changed the interior would be wrong.
    for mode in [&repeat, &decal] {
        assert_eq!(
            pixel(mode, 2, 2),
            pixel(&clamp, 2, 2),
            "a tile mode changed the image's own interior"
        );
    }

    // Outside it they must not. Clamping holds the edge, repeating starts the
    // image again, and decal contributes nothing.
    //
    // "Nothing" here is transparent black rather than the cleared color,
    // because these draw with `Src`, which replaces whatever it covers. Under
    // the source-over a caller would normally use, the same nothing leaves the
    // backdrop untouched, which is what decal is for.
    let outside = (24, 24);
    assert_eq!(
        pixel(&decal, outside.0, outside.1),
        [0, 0, 0, 0],
        "decal drew something outside the image"
    );
    assert_ne!(
        pixel(&clamp, outside.0, outside.1),
        pixel(&decal, outside.0, outside.1),
        "clamp and decal agree outside the image"
    );
    // Repeating puts the image's top-left quadrant back at the start of each
    // tile, where clamping holds the bottom-right one.
    assert_eq!(
        pixel(&repeat, 18, 18),
        [255, 0, 0, 255],
        "repeat did not start the image again"
    );
    assert_eq!(
        pixel(&clamp, 18, 18),
        [255, 255, 0, 255],
        "clamp did not hold the corner texel"
    );

    // Mirroring reflects each alternate copy instead of restarting it. The
    // image covers sixteen target pixels, so the fold is about sixteen, and a
    // fragment samples at its center: 17.5 reflects onto 14.5. That column is
    // inside the image, where every mode agrees, so it is read from `clamp`.
    let mirror = render::<VulkanHal>(&mut ctx, quarter(TileMode::Mirror));
    assert_eq!(
        pixel(&mirror, 2, 2),
        pixel(&clamp, 2, 2),
        "mirror changed the image's own interior"
    );
    assert_eq!(
        pixel(&mirror, 17, 2),
        pixel(&clamp, 14, 2),
        "mirror did not reflect the copy across the edge"
    );
    // And it is a reflection rather than a repeat: the same column under
    // repeating restarts at the image's left edge instead.
    assert_ne!(
        pixel(&mirror, 17, 2),
        pixel(&repeat, 17, 2),
        "mirror and repeat agree where they must differ"
    );
}

#[test]
fn a_batch_naming_a_texture_nobody_supplied_is_refused() {
    let mut ran = 0;
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, full_target_mapping(), BlendMode::Src)
        .expect("push");

    // Reading whatever happens to be bound would draw a plausible picture from
    // a previous frame's texture, which is worse than failing.
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("target");
        let result = ctx.submit_batch(&mut target, &batch, PassDescriptor::preserve());
        ctx.destroy_texture(target);
        assert!(
            result.is_err(),
            "vulkan accepted an unsupplied texture slot"
        );
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("target");
        let result = ctx.submit_batch(&mut target, &batch, PassDescriptor::preserve());
        ctx.destroy_texture(target);
        assert!(result.is_err(), "gles accepted an unsupplied texture slot");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_rendered_target_can_be_sampled_by_a_later_pass() {
    // The case a save layer is built from: draw into a texture, then draw that
    // texture somewhere else. It exercises the layout transition a sampled
    // render target needs, which an uploaded texture does not, since the two
    // arrive in different layouts.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let mut layer = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("layer");
    let mut fill = Batch::new();
    fill.push(
        &FULL,
        &QUAD,
        Material::solid([0.0, 1.0, 0.0, 1.0]),
        BlendMode::Src,
    )
    .expect("push");
    ctx.submit_batch(
        &mut layer,
        &fill,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("draw into the layer");

    let mut composite = Batch::new();
    composite
        .push(
            &FULL,
            &QUAD,
            Material::Image {
                origin: [-1.0, 1.0],
                to_local: [0.5, 0.0, 0.0, -0.5],
                slot: 0,
                alpha: 1.0,
                tile: TileMode::Clamp,
            },
            BlendMode::Src,
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &composite,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&layer],
    )
    .expect("composite");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(layer);

    assert_eq!(
        pixel(&pixels, 16, 16),
        [0, 255, 0, 255],
        "the layer did not survive being sampled"
    );
}

/// A four-by-four single-channel image with a distinct value per texel.
fn coverage_pixels() -> Vec<u8> {
    (0..16u8).map(|i| i * 16 + 8).collect()
}

#[test]
fn a_single_channel_texture_round_trips_on_both_backends() {
    // Coverage is one byte per texel, and storing it four times over costs four
    // times the memory and four times the bandwidth to sample. The transfer
    // paths had the four wired in as a literal, so this is what says they read
    // the format instead: a one-byte format with a four-byte assumption
    // anywhere reads three texels past the end of every row.
    let want = coverage_pixels();
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(SOURCE, PixelFormat::R8Unorm))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(
            got, want,
            "vulkan changed a single-channel image in transit"
        );
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(SOURCE, PixelFormat::R8Unorm))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want, "gles changed a single-channel image in transit");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_single_channel_texture_samples_as_coverage() {
    // The shader reads the red channel, which is where a one-channel texture
    // puts its only value and where a four-channel one repeats it. Sampling one
    // as coverage gives that value alone, which is what the glyph material
    // scales a solid by.
    //
    // Left half fully covered and right half half-covered, so a swapped axis or
    // a wrong row stride is a visibly different picture rather than a uniform
    // one.
    let mut texels = vec![0u8; SOURCE.area() as usize];
    for y in 0..SOURCE.height {
        for x in 0..SOURCE.width {
            texels[(y * SOURCE.width + x) as usize] = if x < SOURCE.width / 2 { 255 } else { 128 };
        }
    }

    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        assert_coverage(&render_coverage::<VulkanHal>(&mut ctx, &texels), "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        assert_coverage(&render_coverage::<GlesHal>(&mut ctx, &texels), "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

/// Full coverage on the left, half on the right, both tinted white.
fn assert_coverage(pixels: &[u8], backend: &str) {
    assert_eq!(
        pixel(pixels, 4, 16),
        [255, 255, 255, 255],
        "{backend}: left"
    );
    let right = pixel(pixels, 27, 16);
    assert!(
        (right[0] as i32 - 128).abs() <= 2 && right[3] == right[0],
        "{backend}: half coverage came back {right:?}"
    );
}

/// Upload single-channel coverage and draw it through the glyph material.
fn render_coverage<H: Hal>(ctx: &mut H::Context, texels: &[u8]) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    use impeller_hal::Vertex;

    let mut source = ctx
        .create_texture(&TextureDescriptor::offscreen(SOURCE, PixelFormat::R8Unorm))
        .expect("source texture");
    ctx.write_texture(&mut source, texels).expect("upload");

    // The glyph material takes its coordinates from the vertices, so the quad
    // carries them rather than the paint.
    let corners = [
        ([-1.0f32, -1.0], [0.0f32, 1.0]),
        ([1.0, -1.0], [1.0, 1.0]),
        ([1.0, 1.0], [1.0, 0.0]),
        ([-1.0, 1.0], [0.0, 0.0]),
    ];
    let vertices: Vec<Vertex> = corners.iter().map(|(p, uv)| Vertex::new(*p, *uv)).collect();

    let mut batch = Batch::new();
    batch
        .push_mesh(
            &vertices,
            &QUAD,
            Material::Glyph {
                color: [1.0, 1.0, 1.0, 1.0],
                slot: 0,
            },
            BlendMode::Src,
            None,
            impeller_hal::ClipState::UNCLIPPED,
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&source],
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(source);
    pixels
}
