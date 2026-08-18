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

use impeller_hal_gles::Validated as GlesValidated;
use impeller_hal_gles::{DisplayTarget, GlesHal};
use impeller_hal_vulkan::Validated;
use impeller_hal_vulkan::{DevicePreference, VulkanHal};
use impeller_testkit::{accepts, compare, corpus, render_scene, Scene};

#[test]
fn the_corpus_matches_across_backends() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    eprintln!(
        "vulkan: {}\ngles:   {}",
        vulkan.capabilities().device_name,
        gles.capabilities().device_name
    );

    let mut compared = 0;
    let mut gaps = Vec::new();
    let mut failures = Vec::new();
    let mut surprises = Vec::new();

    for scene in corpus() {
        // A scene either backend says it cannot render is a declared gap, not a
        // silent skip: the comparison is not available for it, and reporting
        // that is the point. Deciding this from what the scene needs, rather
        // than from whether rendering happened to fail, is what keeps a genuine
        // regression from being absorbed as a gap.
        let on_vulkan = scene.supported_by(vulkan.capabilities());
        let on_gles = scene.supported_by(gles.capabilities());
        if !(on_vulkan && on_gles) {
            let which = match (on_vulkan, on_gles) {
                (false, false) => "either backend",
                (false, true) => "vulkan",
                _ => "gles",
            };
            gaps.push(format!("  {:<26} not available on {which}", scene.name));
            continue;
        }

        let from_vulkan = match render_scene::<VulkanHal>(&mut vulkan, &scene) {
            Ok(image) => image,
            Err(e) => {
                surprises.push(format!("  {}: vulkan refused it: {e}", scene.name));
                continue;
            }
        };
        let from_gles = match render_scene::<GlesHal>(&mut gles, &scene) {
            Ok(image) => image,
            Err(e) => {
                // The scene said this device could render it and the device
                // disagreed. One of the two is wrong, and neither is a gap.
                surprises.push(format!("  {}: gles refused it: {e}", scene.name));
                continue;
            }
        };

        let difference = compare(&from_vulkan, &from_gles).expect("same size");
        if accepts(&difference, scene.tolerance()) {
            eprintln!("  {:<26} {difference}", scene.name);
            compared += 1;
        } else {
            failures.push(format!("  {}: {difference}", scene.name));
        }
    }

    if !gaps.is_empty() {
        eprintln!(
            "{} of {} scene(s) not compared, by declared capability:\n{}",
            gaps.len(),
            corpus().len(),
            gaps.join("\n")
        );
    }

    assert!(
        failures.is_empty(),
        "{} scene(s) diverged between backends:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        surprises.is_empty(),
        "{} scene(s) were refused by a backend that claims to support them:\n{}",
        surprises.len(),
        surprises.join("\n")
    );
    assert_eq!(
        compared + gaps.len(),
        corpus().len(),
        "not every scene was either compared or declared unavailable"
    );
}

#[test]
fn both_backends_agree_on_orientation() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
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

#[test]
fn a_bounded_layer_renders_like_a_full_size_one_on_every_backend() {
    // Bounds tell a layer the region it covers so its target can be smaller
    // than the frame and sit at an offset inside it. That is an optimization,
    // so the pixels must not move -- and this is the whole guarantee, checked
    // against the same scene with its bounds stripped rather than against
    // stored values, so it keeps holding as both change.
    //
    // Per backend rather than only on the first. An offset that a full-size
    // layer hides is exactly the kind of thing the two could differ on, since
    // one encodes a pass into a command buffer and the other rebinds a
    // framebuffer on a global state machine.
    let mut vulkan = Validated::new(DevicePreference::Auto).ok();
    let mut gles = GlesValidated::new(DisplayTarget::Surfaceless).ok();
    if vulkan.is_none() && gles.is_none() {
        eprintln!("skipping: no backend available");
        return;
    }

    let mut checked = 0;
    for scene in corpus().into_iter().filter(Scene::has_bounded_layer) {
        let unbounded = scene.unbounded();
        // Stripping has to have done something, or the two renders are the
        // same recording and the comparison below is vacuous.
        assert!(
            !unbounded.has_bounded_layer(),
            "{} kept its bounds after stripping",
            scene.name
        );

        if let Some(ctx) = vulkan.as_mut() {
            let with = render_scene::<VulkanHal>(ctx, &scene).expect("bounded");
            let without = render_scene::<VulkanHal>(ctx, &unbounded).expect("unbounded");
            let difference = compare(&without, &with).expect("same size");
            assert_eq!(
                difference.max_delta, 0,
                "vulkan moved pixels on {} when the layer was given bounds: {difference:?}",
                scene.name
            );
            checked += 1;
        }
        if let Some(ctx) = gles.as_mut() {
            let with = render_scene::<GlesHal>(ctx, &scene).expect("bounded");
            let without = render_scene::<GlesHal>(ctx, &unbounded).expect("unbounded");
            let difference = compare(&without, &with).expect("same size");
            assert_eq!(
                difference.max_delta, 0,
                "gles moved pixels on {} when the layer was given bounds: {difference:?}",
                scene.name
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no scene in the corpus has a bounded layer, so nothing here was checked"
    );
}
