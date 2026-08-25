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

use impeller_hal::{Hal, HalContext};
use impeller_hal_gles::Validated as GlesValidated;
use impeller_hal_gles::{DisplayTarget, GlesHal};
use impeller_hal_vulkan::Validated;
use impeller_hal_vulkan::{DevicePreference, VulkanHal};
use impeller_testkit::{
    accepts, catalog, compare, corpus, render_scene, Item, Scene, Shape, Tolerance,
};

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
            failures.push(format!(
                "  {}: {}",
                scene.name,
                difference.describe(scene.tolerance())
            ));
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

        // One level per composite rather than bit-exact, and the reason came
        // from a third device rather than from argument. A bounded layer
        // composites from a target of another size at another origin, so the
        // coordinates it samples at differ from a full-size layer's even where
        // the result is mathematically the same -- and two identical products
        // can round to eight bits differently. Each composite multiplies a
        // group alpha and rounds once, so a scene of two nested layers can
        // differ by two.
        //
        // Two is the bound because `layer-nested-clipped` is the deepest scene
        // in the corpus, at an outer bounded layer and an inner one. A scene
        // nesting deeper would want a deeper bound, and would fail here rather
        // than pass quietly, which is the right way round.
        //
        // This was exact and passed on two x86 devices for as long as those
        // were the only ones asked. On a Raspberry Pi 4 it comes back one level
        // apart on Vulkan and two on GLES, across roughly one percent of the
        // pixels -- rounding, not a layer moving anything. A bounded layer
        // landing in the wrong place moves pixels by the whole range.
        let per_composite = Tolerance::new(2, 0.0);
        if let Some(ctx) = vulkan.as_mut() {
            let with = render_scene::<VulkanHal>(ctx, &scene).expect("bounded");
            let without = render_scene::<VulkanHal>(ctx, &unbounded).expect("unbounded");
            let difference = compare(&without, &with).expect("same size");
            assert!(
                accepts(&difference, per_composite),
                "vulkan moved pixels on {} when the layer was given bounds: {difference:?}",
                scene.name
            );
            checked += 1;
        }
        if let Some(ctx) = gles.as_mut() {
            let with = render_scene::<GlesHal>(ctx, &scene).expect("bounded");
            let without = render_scene::<GlesHal>(ctx, &unbounded).expect("unbounded");
            let difference = compare(&without, &with).expect("same size");
            assert!(
                accepts(&difference, per_composite),
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

/// A stated color arrives as the bytes it states, on every backend.
///
/// Everything else in this file is a *relative* check — one backend against
/// another, a scene against itself, a bounded layer against a full-size one.
/// None of them can see a picture that is wrong in a way both implementations
/// agree on, which is exactly what a mistake in the color pipeline produces:
/// every comparison here would pass with the whole corpus a third too bright.
/// `cargo xtask gallery` exists for that, and it needs a person.
///
/// This is the cheap half of what the person does. A scene states its colors as
/// components in sRGB's transfer function, the pipeline carries them in that
/// transfer function untouched, and a plain eight-bit target stores them
/// untransformed — so an opaque fill has to come back at exactly
/// `round(component * 255)`, and so does the background behind it.
///
/// Mid-tones, and that is the whole point of the numbers chosen. Zero and one
/// are fixed points of the transfer function, so a scene of black and white
/// primaries comes back identical whether the pipeline holds light or encoded
/// color and anchors nothing at all.
#[test]
fn a_stated_color_arrives_as_the_bytes_it_states() {
    const FILL: [f32; 4] = [0.27, 0.61, 0.44, 1.0];
    const GROUND: [f32; 4] = [0.71, 0.33, 0.18, 1.0];
    let byte = |c: f32| (c * 255.0).round() as u8;

    // One sample, no antialiasing to soften an edge, and both sample points
    // well away from one.
    let scene = Scene::new(
        "anchor/a-stated-color",
        vec![Item::fill(
            Shape::Rect {
                min: [32.0, 32.0],
                max: [96.0, 96.0],
            },
            FILL,
        )],
    )
    .with_background(GROUND)
    .with_samples(1);

    let mut checked = 0;
    if let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) {
        check_anchor::<VulkanHal>(&mut vulkan, &scene, "vulkan", FILL, GROUND, byte);
        checked += 1;
    }
    if let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) {
        check_anchor::<GlesHal>(&mut gles, &scene, "gles", FILL, GROUND, byte);
        checked += 1;
    }
    assert!(checked > 0, "no backend was available to anchor against");
}

fn check_anchor<H: Hal>(
    ctx: &mut H::Context,
    scene: &Scene,
    backend: &str,
    fill: [f32; 4],
    ground: [f32; 4],
    byte: impl Fn(f32) -> u8,
) where
    H::Context: HalContext<Hal = H>,
{
    let image = render_scene::<H>(ctx, scene).expect("render");
    let inside = image.pixel(64, 64);
    let want = [byte(fill[0]), byte(fill[1]), byte(fill[2]), 255];
    assert_eq!(
        inside, want,
        "{backend}: a fill stated as {fill:?} came back {inside:?}, wanted {want:?}"
    );

    let outside = image.pixel(8, 8);
    let want = [byte(ground[0]), byte(ground[1]), byte(ground[2]), 255];
    assert_eq!(
        outside, want,
        "{backend}: a background stated as {ground:?} came back {outside:?}, wanted {want:?}"
    );
}
