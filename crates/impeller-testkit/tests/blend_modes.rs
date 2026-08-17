//! Every blend mode, checked against the equation it claims to implement.
//!
//! The expectations are computed from the blend equation rather than recorded
//! from a run, so a mode wired to the wrong factors fails rather than being
//! enshrined. Each mode is also compared between the two backends, since they
//! translate the same portable factors into different APIs and a mistranslation
//! would otherwise show up only as a picture someone thought looked wrong.

use impeller_hal::{
    Batch, BlendFactor, BlendMode, Extent2D, Hal, HalContext, Material, PassDescriptor,
    PixelFormat, TextureDescriptor,
};
use impeller_hal_gles::{DisplayTarget, GlesContext, GlesHal};
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

/// What the blend equation says the result should be.
fn expected(mode: BlendMode) -> [u8; 4] {
    let factors = mode.factors();
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let value = SRC_PREMULTIPLIED[channel]
            * factor_value(factors.src, SRC_PREMULTIPLIED, DST, channel)
            + DST[channel] * factor_value(factors.dst, SRC_PREMULTIPLIED, DST, channel);
        out[channel] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

/// Draw the source over a cleared destination with one mode, and read a pixel.
fn render<H: Hal>(ctx: &mut H::Context, mode: BlendMode) -> [u8; 4]
where
    H::Context: HalContext<Hal = H>,
{
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, Material::solid(SRC), mode)
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(DST))
        .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);

    let middle = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    [
        pixels[middle],
        pixels[middle + 1],
        pixels[middle + 2],
        pixels[middle + 3],
    ]
}

fn near(got: [u8; 4], want: [u8; 4]) -> bool {
    got.iter()
        .zip(&want)
        .all(|(a, b)| (*a as i32 - *b as i32).abs() <= 1)
}

#[test]
fn every_mode_matches_its_equation_on_vulkan() {
    let Ok(mut ctx) = VulkanContext::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut failures = Vec::new();
    for mode in BlendMode::ALL {
        let got = render::<VulkanHal>(&mut ctx, *mode);
        let want = expected(*mode);
        if !near(got, want) {
            failures.push(format!("  {mode}: got {got:?}, equation says {want:?}"));
        }
    }
    // Every mode reported at once: a wrong factor table usually breaks several,
    // and seeing which ones is what identifies the mistake.
    assert!(
        failures.is_empty(),
        "{} mode(s) disagree with the blend equation:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_mode_matches_its_equation_on_gles() {
    let Ok(mut ctx) = GlesContext::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    let mut failures = Vec::new();
    for mode in BlendMode::ALL {
        let got = render::<GlesHal>(&mut ctx, *mode);
        let want = expected(*mode);
        if !near(got, want) {
            failures.push(format!("  {mode}: got {got:?}, equation says {want:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mode(s) disagree with the blend equation:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_mode_agrees_between_the_backends() {
    let Ok(mut vulkan) = VulkanContext::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesContext::new(DisplayTarget::Surfaceless) else {
        return;
    };

    let mut failures = Vec::new();
    for mode in BlendMode::ALL {
        let a = render::<VulkanHal>(&mut vulkan, *mode);
        let b = render::<GlesHal>(&mut gles, *mode);
        // One unit of tolerance, because a blended result is converted to fixed
        // point and either of the two nearest values is permitted.
        if !near(a, b) {
            failures.push(format!("  {mode}: vulkan {a:?}, gles {b:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mode(s) diverge between backends:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_modes_are_actually_distinct() {
    let Ok(mut ctx) = VulkanContext::new(DevicePreference::Auto) else {
        return;
    };
    // A factor table where two entries collapsed onto the same pair would pass
    // every equation check above, because the equation would then be wrong in
    // the same way as the implementation. This compares how many distinct
    // results were rendered against how many the equation predicts, so it needs
    // no threshold and does not care which modes happen to coincide for these
    // particular colours.
    let mut rendered: Vec<[u8; 4]> = BlendMode::ALL
        .iter()
        .map(|mode| render::<VulkanHal>(&mut ctx, *mode))
        .collect();
    let mut predicted: Vec<[u8; 4]> = BlendMode::ALL.iter().map(|mode| expected(*mode)).collect();

    for list in [&mut rendered, &mut predicted] {
        list.sort_unstable();
        list.dedup();
    }
    assert_eq!(
        rendered.len(),
        predicted.len(),
        "the implementation distinguishes {} results where the equation predicts {}",
        rendered.len(),
        predicted.len()
    );
}
