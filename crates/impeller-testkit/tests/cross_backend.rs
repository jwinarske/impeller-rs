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

use impeller_hal::{Hal, HalContext, PixelFormat};
use impeller_hal_gles::Validated as GlesValidated;
use impeller_hal_gles::{DisplayTarget, GlesHal};
use impeller_hal_vulkan::Validated;
use impeller_hal_vulkan::{DevicePreference, VulkanHal};
use impeller_testkit::{accepts, catalog, compare, corpus, render_scene, render_scene_into, Scene};

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
    // With one exception, which this test found rather than anticipated: a
    // layer that filters its backdrop is not merely allocating when it states
    // bounds. The bounds are the region that gets filtered, so stripping them
    // blurs the whole frame instead of a panel of it. Both are right, and they
    // are different pictures, so the comparison has nothing to say about them.
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
    for scene in corpus()
        .into_iter()
        .filter(Scene::has_bounded_layer)
        .filter(|scene| !scene.filters_its_backdrop())
    {
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

/// Scenes that leave the most state behind them, on either backend.
///
/// Clips set a scissor and write a stencil, layers bind a target and a texture
/// and change which framebuffer is current, a multisampled pass allocates a
/// transient buffer and resolves through a blit. Each of those is state that
/// belongs to one frame, and each has to be put back.
fn stateful() -> Vec<Scene> {
    corpus()
        .into_iter()
        .filter(|scene| {
            let name = scene.name;
            name.starts_with("clip-") || name.starts_with("layer-") || name.contains("analytic")
        })
        .collect()
}

#[test]
fn a_scene_renders_the_same_after_an_unrelated_frame() {
    // Every device test here renders one scene into a fresh context, which is
    // the one arrangement in which state left behind by a previous frame cannot
    // be seen. A frame loop is the opposite: the same context renders scene
    // after scene, and anything one frame leaves set is what the next one
    // inherits.
    //
    // This is not hypothetical on either backend, and has already happened
    // once: a clip left the scissor enabled, and the multisample resolve --
    // which is a blit, and blits are scissored -- copied only the part of the
    // frame the previous draw could touch. The pixels outside kept whatever
    // they held before, which on a fresh context is a cleared target and in a
    // loop is the last frame.
    //
    // So: render a scene, render something that sets as much state as the
    // corpus can, then render the first scene again and require the two to be
    // identical. Not similar -- identical, because the same recording on the
    // same device has no licence to differ at all.
    //
    // What this guards is the property, not any one mechanism that provides
    // it, and the difference matters. The GLES backend disables the scissor
    // twice, once when a pass begins and once after every resolve, and either
    // alone is enough: deleting one changes nothing here, and deleting both
    // makes this fail by thousands of pixels. A reader who finds one of them
    // apparently redundant and removes it will not be caught by this test --
    // they will be caught by whoever removes the second.
    //
    // It also cannot see a fault that is deterministic within a single frame,
    // since that spoils both renders equally. The scissored-resolve bug above
    // was of exactly that kind, which is why it is guarded by the pixels a
    // scene produces rather than by this.
    let mut vulkan = Validated::new(DevicePreference::Auto).ok();
    let mut gles = GlesValidated::new(DisplayTarget::Surfaceless).ok();
    if vulkan.is_none() && gles.is_none() {
        eprintln!("skipping: no device on either backend");
        return;
    }

    let pollutants = stateful();
    assert!(
        pollutants.len() >= 3,
        "the corpus should carry several state-heavy scenes, found {}",
        pollutants.len()
    );

    let mut checked = 0;
    for scene in corpus() {
        if let Some(ctx) = vulkan.as_mut() {
            if scene.supported_by(ctx.capabilities()) {
                checked += check_repeat::<VulkanHal>(ctx, &scene, &pollutants, "vulkan");
            }
        }
        if let Some(ctx) = gles.as_mut() {
            if scene.supported_by(ctx.capabilities()) {
                checked += check_repeat::<GlesHal>(ctx, &scene, &pollutants, "gles");
            }
        }
    }
    assert!(checked > 0, "nothing was checked");
    eprintln!("{checked} scene renders compared against a repeat");
}

/// Render `scene`, then the pollutants, then `scene` again, and compare.
fn check_repeat<H: Hal>(
    ctx: &mut H::Context,
    scene: &Scene,
    pollutants: &[Scene],
    backend: &str,
) -> usize
where
    H::Context: HalContext<Hal = H>,
{
    let Ok(first) = render_scene::<H>(ctx, scene) else {
        return 0;
    };
    for polluter in pollutants {
        if polluter.supported_by(ctx.capabilities()) {
            let _ = render_scene::<H>(ctx, polluter);
        }
    }
    let again = render_scene::<H>(ctx, scene).expect("the same scene rendered twice");
    let differing = first
        .pixels
        .chunks_exact(4)
        .zip(again.pixels.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{backend}: {} rendered differently after other frames, in {differing} pixel(s)",
        scene.name
    );
    1
}

#[test]
fn a_feature_a_scene_asks_for_has_to_change_the_picture() {
    // The gap every other test here leaves open. Comparing two backends says
    // they agree, and two backends agree perfectly about a feature both of
    // them ignore. Comparing against a stored image would catch it, and there
    // are no stored images here on purpose. So the check is against the same
    // scene with the feature taken out: if the two render the same, the scene
    // asked for something that did nothing.
    //
    // Three plates in a row needed this asked by hand -- a blurred image, a
    // sprite batch combining colors, a flood fill through a clip -- and each
    // time the hand-written check was itself wrong before the renderer was.
    // Asking it of every scene that carries a feature costs one render each
    // and cannot be forgotten.
    let mut vulkan = Validated::new(DevicePreference::Auto).ok();
    let gles = GlesValidated::new(DisplayTarget::Surfaceless).ok();
    if vulkan.is_none() && gles.is_none() {
        eprintln!("skipping: no backend available");
        return;
    }

    let mut checked = 0;
    for scene in catalog()
        .into_iter()
        .chain(corpus())
        .filter(Scene::carries_a_visual_feature)
    {
        let plain = scene.plain();
        // The stripping has to have done something, or the comparison below is
        // between a scene and itself and proves nothing.
        assert!(
            !plain.carries_a_visual_feature(),
            "{} kept its features after stripping",
            scene.name
        );

        if let Some(ctx) = vulkan.as_mut() {
            let with = render_scene::<VulkanHal>(ctx, &scene).expect("featured");
            let without = render_scene::<VulkanHal>(ctx, &plain).expect("plain");
            assert_ne!(
                with.pixels, without.pixels,
                "{} renders the same with its features stripped, so whatever it \
                 asks for is not reaching the picture",
                scene.name
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no scene in either list carries a feature, which cannot be right"
    );
    eprintln!("checked {checked} scene(s) for a feature that does nothing");
}

#[test]
fn the_corpus_matches_across_backends_on_a_target_that_encodes_on_write() {
    // Everything else here renders into linear eight-bit color, so the path
    // where the attachment applies the transfer function on write is exercised
    // by a handful of targeted tests and by nothing else in the corpus. The two
    // backends reach that path by different means -- one asks for an image view
    // in the sRGB form of the format, the other attaches a texture whose own
    // format carries it and has no separate control over whether encoding
    // happens -- and two implementations of one conversion is exactly what a
    // comparison between backends is for.
    //
    // The color policy rests on this: linear light the whole way and only the
    // final write encoded. A backend that converted early, twice, or not at all
    // would still round-trip its own output and would differ from the other one
    // here.
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };

    let mut compared = 0;
    let mut failures = Vec::new();
    for scene in corpus() {
        let (Ok(from_vulkan), Ok(from_gles)) = (
            render_scene_into::<VulkanHal>(&mut vulkan, &scene, PixelFormat::Rgba8UnormSrgb),
            render_scene_into::<GlesHal>(&mut gles, &scene, PixelFormat::Rgba8UnormSrgb),
        ) else {
            // A scene either backend declines is already reported by the
            // comparison above; this one is about the format and adds nothing
            // by repeating it.
            continue;
        };
        let difference = compare(&from_vulkan, &from_gles).expect("same size");
        if !accepts(&difference, scene.tolerance()) {
            failures.push(format!("{}: {difference:?}", scene.name));
        }
        compared += 1;
    }

    // Most of the corpus, not merely some of it: a filter that quietly
    // excluded everything would satisfy a floor of one and prove nothing.
    let total = corpus().len();
    assert!(
        compared * 4 >= total * 3,
        "only {compared} of {total} scenes reached the comparison"
    );
    assert!(
        failures.is_empty(),
        "{} of {compared} scenes differ between the backends when the target \
         encodes on write:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
