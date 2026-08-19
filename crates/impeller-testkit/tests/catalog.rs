//! The catalog rendered, and rendered the same way twice.
//!
//! These scenes mirror Impeller's own playground, which is a mode of its test
//! suite rather than a separate program: a test renders a frame, and with a
//! flag set it opens a window instead of running headless. Ours are data, so
//! the window and the headless run read the same list -- but that only helps
//! if the list is exercised, and a picture nobody renders is a picture nobody
//! knows is broken.
//!
//! Two things are checked, and deliberately not a third. Every scene draws
//! something on each backend, and the two backends agree. What is not checked
//! is whether a scene looks like the one it is named after: that is a judgment
//! about a picture, made by looking at it, which is what the playground is
//! for. The corpus is where per-scene tolerances and a software reference
//! live; this collection is broad rather than exact.

use impeller_hal::{Hal, HalContext};
use impeller_hal_gles::Validated as GlesValidated;
use impeller_hal_gles::{DisplayTarget, GlesHal};
use impeller_hal_vulkan::Validated;
use impeller_hal_vulkan::{DevicePreference, VulkanHal};
use impeller_testkit::{accepts, catalog, compare, render_scene, Image, Scene, Tolerance};

/// What the two backends are allowed to disagree by across the whole catalog.
///
/// One budget for the collection rather than one argued about per picture,
/// which is the difference between this and the corpus. It is looser than any
/// corpus scene's, and deliberately: a plate here draws several shapes and
/// long strokes, so it has an order of magnitude more edge pixels than a
/// corpus scene does, and an edge pixel is exactly where two rasterizers may
/// legitimately assign coverage to a different number of samples. At four
/// samples one of them is a quarter, which is sixty-four in eight bits.
///
/// So this bounds *how much of the picture* may differ, not by how much. Three
/// per cent covers the edges of the busiest plate with room to spare, and
/// nothing that is actually wrong stays inside it: a shape missing on one
/// backend, a stroke placed differently, a colour computed differently, each
/// cover far more of the frame than their own outlines.
const CATALOG: Tolerance = Tolerance::new(1, 0.03);

fn render<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Image
where
    H::Context: HalContext<Hal = H>,
{
    render_scene::<H>(ctx, scene).unwrap_or_else(|e| panic!("{}: {e}", scene.name))
}

#[test]
fn every_catalog_scene_draws_something() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let mut blank = Vec::new();
    for scene in catalog() {
        if !scene.supported_by(ctx.capabilities()) {
            continue;
        }
        let image = render::<VulkanHal>(&mut ctx, &scene);
        // Something other than the ground it cleared to. A scene whose
        // geometry landed offscreen, or whose colour matched the background,
        // is one nobody would notice was wrong by scrolling past it.
        let pixels = image.pixels.as_slice();
        let first = &pixels[0..4];
        if pixels.chunks_exact(4).all(|texel| texel == first) {
            blank.push(scene.name);
        }
    }
    assert!(
        blank.is_empty(),
        "these catalog scenes drew nothing at all: {blank:?}"
    );
}

#[test]
fn the_catalog_matches_across_backends() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };

    let mut failures = Vec::new();
    let mut gaps = Vec::new();
    let mut compared = 0usize;
    for scene in catalog() {
        // A scene needing a capability a device does not have is coverage that
        // was not got, not a difference between backends. The scene derives
        // that need from what it contains, so a plate using an advanced blend
        // on a device without the extension is reported here rather than
        // failing as a regression.
        if !scene.supported_by(vulkan.capabilities()) || !scene.supported_by(gles.capabilities()) {
            gaps.push(scene.name);
            continue;
        }
        let a = render::<VulkanHal>(&mut vulkan, &scene);
        let b = render::<GlesHal>(&mut gles, &scene);
        compared += 1;
        let difference = compare(&a, &b).expect("same size");
        if !accepts(&difference, CATALOG) {
            failures.push(format!("{}: {difference}", scene.name));
        }
    }
    // Said out loud rather than left implicit. A comparison that quietly
    // compared nothing passes, and the number is the only thing that
    // distinguishes that from a comparison that found no differences.
    eprintln!("compared {compared} scene(s)");
    if !gaps.is_empty() {
        eprintln!(
            "{} scene(s) not compared, by declared capability: {}",
            gaps.len(),
            gaps.join(", ")
        );
    }
    assert!(
        failures.is_empty(),
        "catalog scenes differ between backends:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn catalog_names_say_which_file_they_came_from() {
    // The name is the only link back to the test each scene mirrors, so it
    // carries the topic its file is named for. A scene that does not is one
    // whose original nobody can find.
    const TOPICS: [&str; 6] = [
        "basic/",
        "path/",
        "gradient/",
        "clip/",
        "opacity/",
        "blend/",
    ];
    let stray: Vec<&str> = catalog()
        .iter()
        .map(|scene| scene.name)
        .filter(|name| !TOPICS.iter().any(|topic| name.starts_with(topic)))
        .collect();
    assert!(stray.is_empty(), "catalog scenes with no topic: {stray:?}");

    // A plate showing something this renderer has and Impeller's playground
    // does not would break the one property the naming carries: that a name
    // leads back to the test it mirrors. There was such a plate briefly --
    // nearest sampling, which has no counterpart there -- and it was removed
    // rather than named around, because a catalog that is a mirror plus some
    // extras is not a mirror. Anything of ours worth looking at belongs in the
    // corpus or in a live scene.
    assert!(
        !catalog().iter().any(|s| s.name.ends_with("-nearest")),
        "a catalog plate is showing something the original has no test for"
    );

    // And no two scenes share a name, which would make one of them
    // unreachable from the playground and hide it from the sweep above.
    let mut names: Vec<&str> = catalog().iter().map(|s| s.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(count, names.len(), "two catalog scenes share a name");
}
