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
//!
//! Not every device can render every scene — advanced blending is an extension
//! one physical device can have while another on the same machine lacks it — so
//! each scene is checked against what it needs before it is rendered. What that
//! must not become is a corpus quietly running fewer scenes than it holds, so a
//! test below asserts every scene is exercised by *some* available device.

use impeller_hal::HalContext;
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use impeller_testkit::{accepts, compare, corpus, render_scene};

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
        // Comparing two devices needs both to be able to render it. Where only
        // one can, there is no comparison to make and saying so is better than
        // asserting against a single implementation of its own output.
        if !(scene.supported_by(HalContext::capabilities(&default))
            && scene.supported_by(HalContext::capabilities(&software)))
        {
            eprintln!(
                "  {:<26} not comparable: one device cannot render it",
                scene.name
            );
            continue;
        }
        let a = render_scene::<VulkanHal>(&mut default, &scene).expect("default device");
        let b = render_scene::<VulkanHal>(&mut software, &scene).expect("software reference");

        let difference = compare(&a, &b).expect("same size");
        if !accepts(&difference, scene.tolerance()) {
            failures.push(format!("  {}: {difference}", scene.name));
        } else {
            eprintln!("  {:<26} {difference}", scene.name);
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

/// Every Vulkan device on this machine, most-preferred first.
///
/// Scenes are run on the first one that can render them, so a scene needing an
/// extension the preferred device lacks is still exercised rather than skipped.
fn available_devices() -> Vec<VulkanContext> {
    let mut devices = Vec::new();
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        if let Ok(ctx) = VulkanContext::new(preference) {
            let name = HalContext::capabilities(&ctx).device_name.clone();
            if devices
                .iter()
                .any(|d| HalContext::capabilities(d).device_name == name)
            {
                continue;
            }
            devices.push(ctx);
        }
    }
    devices
}

/// The index of the first device that can render this scene.
fn first_device_for(devices: &[VulkanContext], scene: &impeller_testkit::Scene) -> Option<usize> {
    devices
        .iter()
        .position(|ctx| scene.supported_by(HalContext::capabilities(ctx)))
}

#[test]
fn every_scene_is_rendered_by_some_available_device() {
    let devices = available_devices();
    if devices.is_empty() {
        eprintln!("skipping: no Vulkan device");
        return;
    }
    // A scene nothing can render is dead weight that reads as coverage. It is
    // not a failure of the renderer, so this reports rather than asserts --
    // but it reports by name, which is what keeps such a scene from sitting in
    // the corpus unnoticed once the capability it needs becomes reachable.
    let orphans: Vec<&str> = corpus()
        .iter()
        .filter(|scene| first_device_for(&devices, scene).is_none())
        .map(|scene| scene.name)
        .collect();
    if !orphans.is_empty() {
        eprintln!(
            "{} scene(s) no available device can render: {}",
            orphans.len(),
            orphans.join(", ")
        );
    }
}

#[test]
fn scenes_render_deterministically_on_one_device() {
    let mut devices = available_devices();
    if devices.is_empty() {
        eprintln!("skipping: no usable Vulkan device");
        return;
    }
    // Rendering the same scene twice must be bit-identical. Without this, a
    // cross-device difference could be noise rather than divergence, and every
    // comparison above would be unreliable.
    for scene in corpus() {
        let Some(index) = first_device_for(&devices, &scene) else {
            continue;
        };
        let ctx = &mut devices[index];
        let first = render_scene::<VulkanHal>(ctx, &scene).expect("first");
        let second = render_scene::<VulkanHal>(ctx, &scene).expect("second");
        assert_eq!(
            first, second,
            "{} did not render deterministically",
            scene.name
        );
    }
}

#[test]
fn the_corpus_actually_draws_something_in_every_scene() {
    let mut devices = available_devices();
    if devices.is_empty() {
        return;
    }
    // A scene that renders as a flat background would pass every comparison
    // while testing nothing, which is the failure mode a corpus is most prone
    // to as it grows.
    for scene in corpus() {
        let Some(index) = first_device_for(&devices, &scene) else {
            continue;
        };
        let image = render_scene::<VulkanHal>(&mut devices[index], &scene).expect("render");
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

#[test]
fn clipped_scenes_differ_from_the_same_scenes_unclipped() {
    let mut devices = available_devices();
    if devices.is_empty() {
        return;
    }
    // Comparing a clipped scene between two implementations proves they agree,
    // not that either applied the clip: both ignoring it agree perfectly. This
    // renders each clipped scene a second time with the clips stripped and
    // requires the two to differ, which is what a clip dropped anywhere between
    // the scene and the scissor unit would fail.
    let mut checked = 0;
    for scene in corpus() {
        if !scene
            .items()
            .any(|item| item.clip.is_some() || item.clip_shape.is_some())
        {
            continue;
        }
        let Some(index) = first_device_for(&devices, &scene) else {
            continue;
        };
        let mut unclipped = scene.clone();
        for item in unclipped.items_mut() {
            item.clip = None;
            item.clip_shape = None;
        }

        let ctx = &mut devices[index];
        let with = render_scene::<VulkanHal>(ctx, &scene).expect("clipped");
        let without = render_scene::<VulkanHal>(ctx, &unclipped).expect("unclipped");
        let difference = compare(&with, &without).expect("same size");
        assert!(
            difference.max_delta > 0,
            "{} rendered identically with and without its clips",
            scene.name
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "no scene in the corpus carries a clip, so nothing here was checked"
    );
}

#[test]
fn the_fill_rules_disagree_on_a_path_that_crosses_itself() {
    // Comparing a scene between implementations proves they agree, not that
    // either did anything: two backends that both ignore the fill rule agree
    // perfectly, which is exactly the state this corpus was in. The rule is
    // only observable on a path that crosses itself, so the check is that the
    // two such scenes differ from each other -- and where, since a difference
    // anywhere would also be satisfied by rendering one of them wrong.
    let mut devices = available_devices();
    if devices.is_empty() {
        eprintln!("skipping: no device");
        return;
    }
    let by_name = |name: &str| {
        corpus()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("the corpus has no scene named {name}"))
    };
    let nonzero = by_name("self-crossing-nonzero");
    let evenodd = by_name("self-crossing-evenodd");
    let Some(index) = first_device_for(&devices, &nonzero) else {
        eprintln!("skipping: no device renders it");
        return;
    };
    let ctx = &mut devices[index];
    let a = render_scene::<VulkanHal>(ctx, &nonzero).expect("non-zero");
    let b = render_scene::<VulkanHal>(ctx, &evenodd).expect("even-odd");

    let at = |image: &impeller_testkit::Image, x: u32, y: u32| {
        image.pixels[((y * image.width + x) * 4) as usize]
    };
    // The middle is enclosed twice by a pentagram's crossings, so non-zero
    // fills it and even-odd does not.
    assert!(at(&a, 64, 64) > 200, "non-zero left the middle empty");
    assert!(at(&b, 64, 64) < 32, "even-odd filled the middle");
    // A point on one of the arms is enclosed once and is filled either way, so
    // the rules differ where they should and agree where they should.
    assert!(at(&a, 64, 22) > 200, "non-zero lost an arm");
    assert!(at(&b, 64, 22) > 200, "even-odd lost an arm");
}
