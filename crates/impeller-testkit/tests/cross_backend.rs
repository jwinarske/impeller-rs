//! The corpus rendered on both backends.
//!
//! This is what the HAL trait was shaped for. Vulkan encodes a batch into a
//! command buffer; GLES replays it against a global state machine. If the two
//! produce the same pixels from the same scene data, the abstraction is
//! carrying real weight rather than wrapping one backend.
//!
//! It is also the first check that the shader pipeline's two targets agree.
//! Both backends render from one WGSL source translated per backend, and a
//! translation that diverged would show up here rather than as a report from
//! whoever ran the other backend first.

use impeller_hal::HalContext;
use impeller_hal_gles::{DisplayTarget, GlesContext, GlesHal};
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use impeller_testkit::{accepts, compare, corpus, render_scene, Scene, Tolerance};

/// Blending converts intermediate results to fixed-point, where either of the
/// two nearest values is permitted. Everything else must match exactly.
fn tolerance_for(scene: &Scene) -> Tolerance {
    if scene
        .items
        .iter()
        .any(|i| i.blend == impeller_hal::BlendMode::SrcOver)
    {
        Tolerance::ROUNDING
    } else {
        Tolerance::EXACT
    }
}

#[test]
fn the_corpus_matches_across_backends() {
    let Ok(mut vulkan) = VulkanContext::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let Ok(mut gles) = GlesContext::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    eprintln!(
        "vulkan: {}\ngles:   {}",
        HalContext::capabilities(&vulkan).device_name,
        HalContext::capabilities(&gles).device_name
    );

    let mut compared = 0;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();

    for scene in corpus() {
        let from_vulkan = render_scene::<VulkanHal>(&mut vulkan, &scene).expect("vulkan");
        let from_gles = match render_scene::<GlesHal>(&mut gles, &scene) {
            Ok(image) => image,
            Err(e) => {
                // A backend that cannot yet do what a scene needs is recorded
                // rather than passed over silently, so the gap stays visible in
                // the run's output instead of being mistaken for coverage.
                skipped.push(format!("  {}: {e}", scene.name));
                continue;
            }
        };

        let difference = compare(&from_vulkan, &from_gles).expect("same size");
        if accepts(&difference, tolerance_for(&scene)) {
            eprintln!("  {:<22} {difference}", scene.name);
            compared += 1;
        } else {
            failures.push(format!("  {}: {difference}", scene.name));
        }
    }

    if !skipped.is_empty() {
        eprintln!("unsupported on GLES:\n{}", skipped.join("\n"));
    }
    assert!(
        failures.is_empty(),
        "{} scene(s) diverged between backends:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        compared > 0,
        "no scene was actually compared across backends"
    );
}

#[test]
fn both_backends_agree_on_orientation() {
    let Ok(mut vulkan) = VulkanContext::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesContext::new(DisplayTarget::Surfaceless) else {
        return;
    };

    // Framebuffer origins differ between the two APIs, and the usual result is
    // an image flipped on one of them. Agreement comes from the shader
    // translator adjusting clip space per target, with no correction on
    // readback; a scene that is not symmetric top-to-bottom is what proves it.
    let scene = corpus()
        .into_iter()
        .find(|s| s.name == "concave-polygon")
        .expect("scene");

    let a = render_scene::<VulkanHal>(&mut vulkan, &scene).expect("vulkan");
    let b = render_scene::<GlesHal>(&mut gles, &scene).expect("gles");

    // Compare against the vertically mirrored image too: if that matched
    // instead, orientation would be wrong in a way a symmetric scene hides.
    let mut mirrored = b.clone();
    let stride = (b.width * 4) as usize;
    for row in 0..b.height as usize {
        let src = (b.height as usize - 1 - row) * stride;
        mirrored.pixels[row * stride..(row + 1) * stride]
            .copy_from_slice(&b.pixels[src..src + stride]);
    }

    assert_eq!(a, b, "backends disagree on orientation");
    assert_ne!(a, mirrored, "the scene is symmetric and proves nothing");
}
