//! Every blend mode, checked against the equation it claims to implement.
//!
//! The expectations are computed from the blend equation rather than recorded
//! from a run, so a mode wired to the wrong factors — or to the wrong blend op
//! — fails rather than being enshrined. The two families are checked the same
//! way but not with the same colors, because what distinguishes the modes
//! within a family differs: Porter-Duff modes differ in *where* each side
//! survives, so they need two partly transparent sides, while the separable
//! modes differ in *how* the two mix per channel, so they need channels that
//! are all distinct and none of them zero.

use googletest::prelude::*;
use impeller_hal::{
    blend::blend_advanced, Batch, BlendFactor, BlendMode, Error, Extent2D, Hal, HalContext,
    Material, PassDescriptor, PixelFormat, TextureDescriptor,
};
use impeller_hal_gles::Validated as GlesValidated;
use impeller_hal_gles::{DisplayTarget, GlesHal};
use impeller_hal_vulkan::Validated;
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};

const SIZE: Extent2D = Extent2D {
    width: 16,
    height: 16,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Straight-alpha source, as a caller writes it.
///
/// Both sides are deliberately partly transparent, and with different alphas.
/// Porter-Duff modes are distinguished by *where* each side survives, so an
/// opaque destination collapses most of them onto each other — source-in
/// becomes source, destination-over becomes destination, and so on. Alphas of
/// 0.4 and 0.75 also keep each factor distinct from its complement, which an
/// alpha of 0.5 would not.
const SRC: [f32; 4] = [1.0, 0.0, 0.0, 0.4];
/// Premultiplied source, as the shader emits it and the equation sees it.
const SRC_PREMULTIPLIED: [f32; 4] = [0.4, 0.0, 0.0, 0.4];
/// The destination, cleared before the draw. Already premultiplied.
const DST: [f32; 4] = [0.0, 0.0, 0.75, 0.75];

/// Straight-alpha source for the separable modes.
///
/// A red source over a blue destination would send multiply, darken and several
/// others to the same near-black result, because a separable function of two
/// channels where one is zero is usually zero. Every channel here is distinct
/// and away from both ends, which is also what keeps dodge and burn off their
/// saturating branches where they would agree with screen and multiply.
/// Both sides are near-opaque rather than half transparent, and that is about
/// sensitivity rather than realism. The composite scales a blend function's
/// contribution by the product of the two alphas, so at a half each the
/// difference between a right formula and a wrong one arrives at the target
/// attenuated to about a third — small enough that a two-percent error in a
/// luminosity landed inside the one-unit tolerance and passed. Near one, almost
/// all of it survives, and the premultiplied round trip is still exercised
/// because neither side is actually opaque.
/// The channels also stay clear of where the formulas turn. Dodge and burn
/// clamp at a ratio of one, hard-light and overlay switch branches at a half,
/// and soft-light switches at a quarter; a channel sitting on one of those is a
/// knife edge where the hardware's un-premultiply and this one's need only
/// disagree in their last bit to fall on opposite sides. The first destination
/// tried here put burn exactly on its clamp in two channels and disagreed by
/// four units for that reason alone.
const ADV_SRC: [f32; 4] = [0.9, 0.4, 0.15, 0.92];
/// The destination for the advanced modes, premultiplied at alpha 0.88.
const ADV_DST: [f32; 4] = [0.264, 0.396, 0.616, 0.88];

/// Evaluate a factor the way the hardware does.
fn factor_value(factor: BlendFactor, src: [f32; 4], dst: [f32; 4], channel: usize) -> f32 {
    match factor {
        BlendFactor::Zero => 0.0,
        BlendFactor::One => 1.0,
        BlendFactor::SrcAlpha => src[3],
        BlendFactor::OneMinusSrcAlpha => 1.0 - src[3],
        BlendFactor::DstAlpha => dst[3],
        BlendFactor::OneMinusDstAlpha => 1.0 - dst[3],
        BlendFactor::DstColor => dst[channel],
    }
}

fn quantize(value: [f32; 4]) -> [u8; 4] {
    let mut out = [0u8; 4];
    for channel in 0..4 {
        out[channel] = (value[channel].clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

/// The source and destination a mode is exercised with, premultiplied.
///
/// The straight-alpha source is what gets handed to the material; the
/// premultiplied one is what the shader emits and the equation sees.
fn colors(mode: BlendMode) -> ([f32; 4], [f32; 4], [f32; 4]) {
    if mode.is_advanced() {
        let a = ADV_SRC[3];
        let premultiplied = [ADV_SRC[0] * a, ADV_SRC[1] * a, ADV_SRC[2] * a, a];
        (ADV_SRC, premultiplied, ADV_DST)
    } else {
        (SRC, SRC_PREMULTIPLIED, DST)
    }
}

/// What the blend equation says the result should be.
fn expected(mode: BlendMode) -> [u8; 4] {
    let (_, src, dst) = colors(mode);
    match mode.factors() {
        Some(factors) => {
            let mut out = [0.0f32; 4];
            for (channel, slot) in out.iter_mut().enumerate() {
                *slot = src[channel] * factor_value(factors.src, src, dst, channel)
                    + dst[channel] * factor_value(factors.dst, src, dst, channel);
            }
            quantize(out)
        }
        // The separable formula, from the specification, evaluated on the same
        // premultiplied inputs the hardware is given.
        None => quantize(blend_advanced(mode, src, dst).expect("advanced formula")),
    }
}

/// Draw the source over a cleared destination with one mode, and read a pixel.
fn render<H: Hal>(ctx: &mut H::Context, mode: BlendMode) -> Result<[u8; 4], Error>
where
    H::Context: HalContext<Hal = H>,
{
    let (straight, _, dst) = colors(mode);
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, Material::solid(straight), mode)
        .expect("push");

    let mut target =
        ctx.create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))?;
    let submitted = ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(dst));
    let result = submitted.and_then(|()| ctx.read_texture(&mut target));
    ctx.destroy_texture(target);
    let pixels = result?;

    let middle = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    Ok([
        pixels[middle],
        pixels[middle + 1],
        pixels[middle + 2],
        pixels[middle + 3],
    ])
}

/// One eighth-bit unit, for every mode in both families.
///
/// A blended result is converted to fixed point and either of the two nearest
/// values is permitted, which is the whole of the allowance. The separable
/// modes were expected to need more, since they un-premultiply before blending
/// and re-premultiply afterwards and soft-light adds a square root on top; they
/// measure at one unit on both physical devices here, so they get one unit. A
/// looser bound would have to be justified by a device that actually needs it,
/// and until then it would only hide a wrong formula: a three-percent error in
/// darken passes at two units and fails at one.
const TOLERANCE: i32 = 1;

fn near(got: [u8; 4], want: [u8; 4]) -> bool {
    got.iter()
        .zip(&want)
        .all(|(a, b)| (*a as i32 - *b as i32).abs() <= TOLERANCE)
}

/// Check a backend against the equation, for whichever modes it supports.
fn check_against_equation<H: Hal>(ctx: &mut H::Context, backend: &str)
where
    H::Context: HalContext<Hal = H>,
{
    let advanced = ctx.capabilities().advanced_blend;
    let mut checked = 0;
    for mode in BlendMode::ALL {
        if mode.is_advanced() && !advanced {
            continue;
        }
        match render::<H>(ctx, *mode) {
            Ok(got) => {
                checked += 1;
                let want = expected(*mode);
                // Every mode reported at once: a wrong factor table usually
                // breaks several, and seeing which ones is what identifies the
                // mistake. A fatal assertion here would name one and stop.
                expect_true!(
                    near(got, want),
                    "{backend} {mode}: got {got:?}, equation says {want:?}"
                );
            }
            Err(e) => {
                expect_true!(
                    false,
                    "{backend} {mode}: refused despite being supported: {e}"
                )
            }
        }
    }
    assert!(
        checked >= BlendMode::PORTER_DUFF.len(),
        "{backend} checked only {checked} modes; every device must do all of Porter-Duff"
    );
}

#[gtest]
fn every_mode_matches_its_equation_on_vulkan() {
    // Both devices, because they do not offer the same modes. Advanced blending
    // is an extension one physical device can have and another can lack on the
    // same machine, and testing only whichever one `Auto` prefers means the
    // separable modes go unchecked wherever the preferred device is the one
    // without it. Which device supplies the coverage is not asserted: that is a
    // property of the machine, not of the renderer.
    let mut ran = false;
    for (preference, label) in [
        (DevicePreference::Auto, "vulkan/auto"),
        (DevicePreference::Software, "vulkan/software"),
    ] {
        let Ok(mut ctx) = Validated::new(preference) else {
            continue;
        };
        check_against_equation::<VulkanHal>(&mut ctx, label);
        ran = true;
    }
    if !ran {
        eprintln!("skipping: no Vulkan device");
    }
}

#[gtest]
fn every_mode_matches_its_equation_on_gles() {
    let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    check_against_equation::<GlesHal>(&mut ctx, "gles");
}

#[test]
fn a_mode_the_device_cannot_do_is_refused_rather_than_approximated() {
    // The failure this guards against is not an error but a picture: a backend
    // that reports no advanced blending and then draws plain source-over
    // anyway looks almost right, and nobody finds that by looking. Both
    // backends run the same check, so whichever one lacks the capability here
    // exercises it.
    let mut ran_on = Vec::new();

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        if !ctx.capabilities().advanced_blend {
            for mode in BlendMode::ADVANCED {
                assert!(
                    matches!(
                        render::<VulkanHal>(&mut ctx, *mode),
                        Err(Error::Unsupported(_))
                    ),
                    "vulkan accepted {mode} while reporting no advanced blending"
                );
            }
            ran_on.push("vulkan");
        }
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        if !ctx.capabilities().advanced_blend {
            for mode in BlendMode::ADVANCED {
                assert!(
                    matches!(
                        render::<GlesHal>(&mut ctx, *mode),
                        Err(Error::Unsupported(_))
                    ),
                    "gles accepted {mode} while reporting no advanced blending"
                );
            }
            ran_on.push("gles");
        }
    }

    // Not an assertion that some backend must lack the capability — on a
    // machine where both have it there is nothing here to check, and saying so
    // is better than reporting a pass that examined nothing.
    if ran_on.is_empty() {
        eprintln!("skipping: every available backend supports advanced blending");
    }
}

#[gtest]
fn the_porter_duff_modes_agree_between_the_backends() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };

    // Only the modes both backends have. The separable modes are Vulkan-only
    // here, and comparing them across backends is what the corpus does through
    // an explicit capability gate rather than what this test papers over.
    for mode in BlendMode::PORTER_DUFF {
        let a = render::<VulkanHal>(&mut vulkan, *mode).expect("vulkan");
        let b = render::<GlesHal>(&mut gles, *mode).expect("gles");
        expect_true!(near(a, b), "{mode}: vulkan {a:?}, gles {b:?}");
    }
}

#[test]
fn the_modes_are_actually_distinct() {
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        let Ok(ctx) = Validated::new(preference) else {
            continue;
        };
        check_distinctness(ctx);
    }
}

fn check_distinctness(mut ctx: Validated) {
    // A table where two entries collapsed onto the same factors — or two modes
    // onto the same blend op — would pass every equation check above, because
    // the equation would then be wrong in the same way as the implementation.
    // This compares how many distinct results were rendered against how many
    // the equation predicts, so it needs no threshold and does not care which
    // modes happen to coincide for these particular colors.
    //
    // The two families are counted separately because they are exercised with
    // different colors, and a result from one carries no information about
    // whether a result from the other is distinct.
    let advanced = ctx.capabilities().advanced_blend;
    let families: &[(&str, &[BlendMode])] = if advanced {
        &[
            ("porter-duff", BlendMode::PORTER_DUFF),
            ("separable", BlendMode::ADVANCED),
        ]
    } else {
        &[("porter-duff", BlendMode::PORTER_DUFF)]
    };

    for (name, modes) in families {
        let mut rendered: Vec<[u8; 4]> = modes
            .iter()
            .map(|mode| render::<VulkanHal>(&mut ctx, *mode).expect("render"))
            .collect();
        let mut predicted: Vec<[u8; 4]> = modes.iter().map(|mode| expected(*mode)).collect();
        for list in [&mut rendered, &mut predicted] {
            list.sort_unstable();
            list.dedup();
        }
        assert_eq!(
            rendered.len(),
            predicted.len(),
            "the {name} modes render {} distinct results where the equation predicts {}",
            rendered.len(),
            predicted.len()
        );
    }
}

/// Two overlapping draws, as one batch and as two submissions.
///
/// `PassDescriptor::preserve` between submissions is a hard ordering: the
/// second draw cannot start before the first has landed. Within one batch there
/// is no such barrier, and the hardware is only obliged to produce the same
/// answer because the device reported coherent advanced blending. This renders
/// both ways and requires them to agree, which is what turns that reported
/// feature into something checked rather than believed — a driver blending the
/// second shape against the original backdrop instead of against the first
/// shape's result would differ exactly in the overlap.
fn overlapping_draws_two_ways(ctx: &mut VulkanContext, mode: BlendMode) -> (Vec<u8>, Vec<u8>) {
    // Two rectangles sharing a middle band, so part of each lies over bare
    // backdrop and part over the other.
    const LEFT: [[f32; 2]; 4] = [[-0.9, -0.6], [0.3, -0.6], [0.3, 0.6], [-0.9, 0.6]];
    const RIGHT: [[f32; 2]; 4] = [[-0.3, -0.6], [0.9, -0.6], [0.9, 0.6], [-0.3, 0.6]];
    let first = [0.85, 0.45, 0.2, 0.75];
    let second = [0.3, 0.65, 0.9, 0.75];

    let render_with = |ctx: &mut VulkanContext, batched: bool| {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("texture");
        if batched {
            let mut batch = Batch::new();
            batch
                .push(&LEFT, &QUAD, Material::solid(first), mode)
                .expect("push");
            batch
                .push(&RIGHT, &QUAD, Material::solid(second), mode)
                .expect("push");
            ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(ADV_DST))
                .expect("one batch");
        } else {
            for (geometry, color, pass) in [
                (&LEFT, first, PassDescriptor::clear(ADV_DST)),
                (&RIGHT, second, PassDescriptor::preserve()),
            ] {
                let mut batch = Batch::new();
                batch
                    .push(geometry, &QUAD, Material::solid(color), mode)
                    .expect("push");
                ctx.submit_batch(&mut target, &batch, pass)
                    .expect("separate batch");
            }
        }
        let pixels = ctx.read_texture(&mut target).expect("readback");
        ctx.destroy_texture(target);
        pixels
    };

    let batched = render_with(ctx, true);
    let separate = render_with(ctx, false);
    (batched, separate)
}

#[test]
fn advanced_blending_is_coherent_within_a_batch() {
    let mut ran = false;
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        let Ok(mut ctx) = Validated::new(preference) else {
            continue;
        };
        if !ctx.capabilities().advanced_blend {
            continue;
        }
        for mode in BlendMode::ADVANCED {
            let (batched, separate) = overlapping_draws_two_ways(&mut ctx, *mode);
            let differing = batched
                .chunks_exact(4)
                .zip(separate.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                differing, 0,
                "{mode}: {differing} pixel(s) differ between one batch and two \
                 submissions, so blending is not coherent within a batch"
            );
            ran = true;
        }
    }
    if !ran {
        eprintln!("skipping: no device reports advanced blending");
    }
}

#[test]
fn overlapping_advanced_draws_actually_blend_with_each_other() {
    // The test above compares two renderings that would both be wrong if the
    // second shape ignored the first. This checks the overlap is genuinely a
    // three-way result: it must match neither shape drawn alone over the
    // backdrop, or the scene proves nothing about ordering.
    let Some(mut ctx) = [DevicePreference::Auto, DevicePreference::Software]
        .into_iter()
        .filter_map(|p| Validated::new(p).ok())
        .find(|ctx| ctx.capabilities().advanced_blend)
    else {
        eprintln!("skipping: no device reports advanced blending");
        return;
    };

    let (batched, _) = overlapping_draws_two_ways(&mut ctx, BlendMode::Multiply);
    let row = (SIZE.height / 2) as usize * SIZE.width as usize * 4;
    // A column inside the shared band, and one inside the left shape only.
    let overlap = &batched[row + 8 * 4..row + 8 * 4 + 4];
    let left_only = &batched[row + 2 * 4..row + 2 * 4 + 4];
    assert_ne!(
        overlap, left_only,
        "the overlap matches the region covered by only one shape"
    );
}
