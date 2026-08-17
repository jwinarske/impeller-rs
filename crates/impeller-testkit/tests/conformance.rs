//! Cross-device conformance over the corpus.
//!
//! Every scene is rendered on the default device and on the software reference,
//! and the results must agree. This is the check that scales: adding a scene
//! extends coverage across every device the corpus runs on, without writing an
//! assertion about what the scene should look like.
//!
//! It is deliberately not a golden test. Comparing two implementations catches
//! anything driver-specific without anyone having to certify a reference image
//! first, which matters while the renderer is still changing what correct output
//! looks like.

use impeller_hal::HalContext;
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use impeller_testkit::{accepts, compare, corpus, render_scene, Scene, Tolerance};

/// Tolerance for a scene, by what it exercises.
///
/// Blending and multisampled resolve both convert intermediate results to
/// normalized fixed-point, where the specification permits either of the two
/// nearest values. Everything else is required to match exactly.
fn tolerance_for(scene: &Scene) -> Tolerance {
    let blends = scene
        .items
        .iter()
        .any(|i| i.blend == impeller_hal::BlendMode::SrcOver);
    if blends || scene.samples > 1 {
        Tolerance::ROUNDING
    } else {
        Tolerance::EXACT
    }
}

fn devices() -> Option<(VulkanContext, VulkanContext)> {
    let default = VulkanContext::new(DevicePreference::Auto).ok()?;
    let software = match VulkanContext::new(DevicePreference::Software) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no software reference ({e})");
            return None;
        }
    };
    if HalContext::capabilities(&default).device_name
        == HalContext::capabilities(&software).device_name
    {
        eprintln!("skipping: the default device is the software reference");
        return None;
    }
    Some((default, software))
}

#[test]
fn every_scene_agrees_across_devices() {
    let Some((mut default, mut software)) = devices() else {
        return;
    };
    eprintln!(
        "comparing {} against {}",
        HalContext::capabilities(&default).device_name,
        HalContext::capabilities(&software).device_name
    );

    let mut failures = Vec::new();
    for scene in corpus() {
        let a = render_scene::<VulkanHal>(&mut default, &scene).expect("default device");
        let b = render_scene::<VulkanHal>(&mut software, &scene).expect("software reference");

        let difference = compare(&a, &b).expect("same size");
        let tolerance = tolerance_for(&scene);
        if !accepts(&difference, tolerance) {
            failures.push(format!("  {}: {difference}", scene.name));
        } else {
            eprintln!("  {:<22} {difference}", scene.name);
        }
    }

    // Reporting every failure rather than the first: one broken capability
    // should not hide the state of the rest of the corpus.
    assert!(
        failures.is_empty(),
        "{} scene(s) diverged across devices:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn scenes_render_deterministically_on_one_device() {
    let Some(mut ctx) = VulkanContext::new(DevicePreference::Auto).ok() else {
        eprintln!("skipping: no usable Vulkan device");
        return;
    };
    // Rendering the same scene twice must be bit-identical. Without this, a
    // cross-device difference could be noise rather than divergence, and every
    // comparison above would be unreliable.
    for scene in corpus() {
        let first = render_scene::<VulkanHal>(&mut ctx, &scene).expect("first");
        let second = render_scene::<VulkanHal>(&mut ctx, &scene).expect("second");
        assert_eq!(
            first, second,
            "{} did not render deterministically",
            scene.name
        );
    }
}

#[test]
fn the_corpus_actually_draws_something_in_every_scene() {
    let Some(mut ctx) = VulkanContext::new(DevicePreference::Auto).ok() else {
        return;
    };
    // A scene that renders as a flat background would pass every comparison
    // while testing nothing, which is the failure mode a corpus is most prone
    // to as it grows.
    for scene in corpus() {
        let image = render_scene::<VulkanHal>(&mut ctx, &scene).expect("render");
        let background = to_bytes(scene.background);
        let drawn = image
            .pixels
            .chunks_exact(4)
            .filter(|p| *p != background)
            .count();
        let fraction = drawn as f32 / image.pixel_count() as f32;
        assert!(
            fraction > 0.02,
            "{} covered only {:.2}% of its target; it may be testing nothing",
            scene.name,
            fraction * 100.0
        );
    }
}

#[test]
fn antialiased_scenes_differ_from_their_aliased_counterparts() {
    let Some(mut ctx) = VulkanContext::new(DevicePreference::Auto).ok() else {
        return;
    };
    if !HalContext::capabilities(&ctx).sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    // The corpus carries a circle at one sample and at four. If they came back
    // identical, the sample count would be reaching neither the pass nor the
    // pipeline, and every antialiased scene would be silently untested.
    let scenes = corpus();
    let aliased = scenes
        .iter()
        .find(|s| s.name == "circle-fill")
        .expect("scene");
    let smooth = scenes
        .iter()
        .find(|s| s.name == "circle-antialiased")
        .expect("scene");

    let a = render_scene::<VulkanHal>(&mut ctx, aliased).expect("aliased");
    let b = render_scene::<VulkanHal>(&mut ctx, smooth).expect("antialiased");
    let difference = compare(&a, &b).expect("same size");
    assert!(
        difference.max_delta > 0,
        "the antialiased scene matched the aliased one exactly"
    );

    // And the difference must be confined to edges rather than the whole shape.
    assert!(
        difference.outlier_fraction() < 0.25,
        "antialiasing changed {:.1}% of pixels; that is more than an edge",
        difference.outlier_fraction() * 100.0
    );
}

fn to_bytes(color: [f32; 4]) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (i, c) in color.iter().enumerate() {
        out[i] = (c * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    out
}
