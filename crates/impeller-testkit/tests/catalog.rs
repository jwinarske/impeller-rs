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

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::panic)]

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
/// backend, a stroke placed differently, a color computed differently, each
/// cover far more of the frame than their own outlines.
const CATALOG: Tolerance = Tolerance::new(1, 0.03);

/// The budget above is the wrong shape for a scene that blends additively, and
/// the two are worth keeping apart rather than widening one to cover both.
///
/// It bounds how much of a picture may differ, on the reasoning that what
/// differs is edges. An additive scene's disagreement is not on its edges: every
/// overlapping draw contributes its own rounding and they add rather than
/// replace, so what differs is the whole overlapping *area* and what bounds it
/// is the magnitude. Judging that by area asks the wrong question, and the
/// additive atlas plate answers it at 3.57 per cent -- just past a budget sized
/// for outlines, by a mechanism that has nothing to do with them.
///
/// So an additive scene is judged on magnitude instead. Found on a Raspberry
/// Pi 5, where the two backends read two levels apart on that plate while
/// sharing a GPU and its fixed-function blending -- so the difference is the
/// two shader compilers, and the blend adding it up.
///
/// This is not the loosening it reads as, and the direction is worth stating
/// because it is the opposite of what swapping in a wider-sounding tolerance
/// suggests. The budget above admits three per cent of the frame differing by
/// *any* amount, up to and including a shape drawn on one backend and missing
/// on the other, as long as it is small enough. The one below admits none: no
/// pixel may differ by more than three levels, anywhere. Four plates take this
/// branch and all four hold to it on a Pi 5, where the two backends are v3d and
/// llvmpipe and share nothing but the frame -- so on the scenes that take it,
/// this is the stricter of the two.
fn tolerance_for(scene: &Scene) -> Tolerance {
    match scene.blends_additively() {
        true => Tolerance::ACCUMULATED,
        false => CATALOG,
    }
}

fn render<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Image
where
    H::Context: HalContext<Hal = H>,
{
    render_scene::<H>(ctx, scene).unwrap_or_else(|e| panic!("{}: {e}", scene.name))
}

#[test]
fn every_catalog_scene_draws_something() {
    // Every Vulkan device on this machine rather than the preferred one, and
    // the difference is not small. Advanced blending is an extension a discrete
    // or integrated GPU can lack while the software rasterizer beside it has
    // it, and nineteen plates of this catalog need it -- so asking only the
    // preferred device skipped nineteen scenes here, silently, and reported a
    // pass. The count in `docs/playground-parity.md` said what the catalog
    // holds and nothing said what had been looked at.
    let mut devices: Vec<Validated> = Vec::new();
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        if let Ok(ctx) = Validated::new(preference) {
            let name = ctx.capabilities().device_name.clone();
            if devices.iter().any(|d| d.capabilities().device_name == name) {
                continue;
            }
            devices.push(ctx);
        }
    }
    if devices.is_empty() {
        eprintln!("skipping: no Vulkan device");
        return;
    }

    let mut blank = Vec::new();
    let mut orphans = Vec::new();
    let mut drawn = 0usize;
    for scene in catalog() {
        let Some(index) = devices
            .iter()
            .position(|ctx| scene.supported_by(ctx.capabilities()))
        else {
            // Reported by name rather than asserted, the way the corpus reports
            // its own: a scene nothing here can render is not a failure of the
            // renderer, and naming it is what keeps it from reading as coverage.
            orphans.push(scene.name);
            continue;
        };
        let ctx = &mut devices[index];
        drawn += 1;
        let image = render::<VulkanHal>(ctx, &scene);
        // Something other than the ground it cleared to. A scene whose
        // geometry landed offscreen, or whose color matched the background,
        // is one nobody would notice was wrong by scrolling past it.
        //
        // Against the background rather than against the first pixel, which is
        // not the same question and gets one plate wrong: a scene that covers
        // the frame twice and blends the second over the first is *correctly*
        // one color everywhere, and reads as blank to a check that only asks
        // whether the picture is uniform. It went unnoticed because the plate
        // in question needs advanced blending, which the device here does not
        // have -- so the flaw only surfaced on a software device that does.
        let ground: [u8; 4] = scene
            .background
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let pixels = image.pixels.as_slice();
        if pixels.chunks_exact(4).all(|texel| texel == ground) {
            blank.push(scene.name);
        }
    }
    eprintln!(
        "drew {drawn} of {} catalog scenes across {} device(s)",
        catalog().len(),
        devices.len()
    );
    if !orphans.is_empty() {
        eprintln!(
            "{} scene(s) no available device can render: {}",
            orphans.len(),
            orphans.join(", ")
        );
    }
    assert!(
        blank.is_empty(),
        "these catalog scenes drew nothing at all: {blank:?}"
    );
}

#[test]
fn the_catalog_matches_across_backends() {
    let mut vulkan = match Validated::new(DevicePreference::Auto) {
        Ok(ctx) => ctx,
        // With the reason. A board with a driver the loader can list and not
        // open reports one thing here and something quite different from the
        // error, and "no Vulkan device" sent one such run looking for a device
        // that was plugged in and working.
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            return;
        }
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
        if !accepts(&difference, tolerance_for(&scene)) {
            failures.push(format!(
                "{}: {}",
                scene.name,
                difference.describe(tolerance_for(&scene))
            ));
        }
    }
    // Said out loud rather than left implicit. A comparison that quietly
    // compared nothing passes, and the number is the only thing that
    // distinguishes that from a comparison that found no differences.
    eprintln!(
        "compared {compared} of {} catalog scenes across backends",
        catalog().len()
    );
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
fn a_scene_that_needs_a_fixture_renders_on_its_own() {
    // A catalog hides an ordering bug by construction: every scene shares one
    // context, so whatever the first scene set up is there for the rest. The
    // fixture programs were registered by a derivation that decided which
    // scenes wanted them, that derivation missed a case, and the plates it
    // missed drew correctly anyway -- another scene having registered them
    // first. Alone they failed every time.
    //
    // So these render one scene per context. Not all of them, which would cost
    // two hundred and forty-nine devices: one for each way a scene can depend
    // on something the executor sets up, which is a program named by a fill, by
    // an image filter, by a group's backdrop, and a texture read from the
    // fixture sheet.
    let wanted = [
        "effect/can-render-runtime-effect",
        "effect/can-render-runtime-effect-filter",
        "effect/clipped-backdrop-filter-with-shader",
        "atlas/draw-atlas-no-color",
    ];
    let mut checked = 0usize;
    for scene in catalog() {
        if !wanted.contains(&scene.name) {
            continue;
        }
        let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
            eprintln!("skipping: no Vulkan device");
            return;
        };
        if !scene.supported_by(ctx.capabilities()) {
            continue;
        }
        render_scene::<VulkanHal>(&mut ctx, &scene)
            .unwrap_or_else(|e| panic!("{} does not render on its own: {e}", scene.name));
        checked += 1;
    }
    assert_eq!(
        checked,
        wanted.len(),
        "{} of the {} scenes named here were not found or not runnable, so this \
         checked less than it says",
        wanted.len() - checked,
        wanted.len()
    );
}

#[test]
fn catalog_names_say_which_file_they_came_from() {
    // The name is the only link back to the test each scene mirrors, so it
    // carries the topic its file is named for. A scene that does not is one
    // whose original nobody can find.
    const TOPICS: [&str; 13] = [
        "basic/",
        "path/",
        "gradient/",
        "clip/",
        "opacity/",
        "blend/",
        "vertices/",
        "atlas/",
        "blur/",
        "shadow/",
        "dl/",
        "effect/",
        // The text file's subject is text rendering, of which only shaping is
        // out of scope here. These plates mirror what a run does on its way to
        // the screen using synthetic coverage, which is the part of that file
        // this renderer has anything to say about.
        "text/",
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

/// Strip every backdrop key from a scene's groups, in place.
fn forget_backdrop_ids(nodes: &mut [impeller_testkit::Node]) {
    for node in nodes {
        if let impeller_testkit::Node::Layer {
            layer, children, ..
        } = node
        {
            layer.backdrop_id = None;
            forget_backdrop_ids(children);
        }
    }
}

#[test]
fn a_backdrop_key_plate_would_notice_if_the_key_stopped_working() {
    // The failure this is here for has happened once already, in the plates
    // right beside these: two backdrop-blur scenes rendered identically to
    // themselves with the blur removed, so they asked for the filter and could
    // not show it. A plate that cannot tell whether the thing it is named for
    // happened is worse than no plate, because the catalog reports it as
    // covered.
    //
    // Nothing else in the catalog can catch this. Cross-backend comparison
    // says the two backends agree, and they would agree just as well on the
    // wrong picture. So each keyed plate is rendered again with its keys taken
    // away, and has to come out different: with them, every panel filters the
    // capture taken before any of them drew; without, each captures afresh and
    // the later ones filter what the earlier ones left behind.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    // The single-panel plate is excluded by name rather than by accident. One
    // layer naming a key has nothing to share with, so it is *supposed* to
    // render the same either way -- and that is the thing it is in the catalog
    // to show.
    let alone = "blur/backdrop-blur-with-single-backdrop-id";
    let mut checked = 0usize;
    for scene in catalog() {
        if !scene.name.contains("backdrop-id") || scene.name == alone {
            continue;
        }
        if !scene.supported_by(ctx.capabilities()) {
            eprintln!("skipping: {} is not supported here", scene.name);
            continue;
        }
        let keyed = render::<VulkanHal>(&mut ctx, &scene);

        let mut without = scene.clone();
        forget_backdrop_ids(&mut without.items);
        assert_ne!(
            scene.items, without.items,
            "{} was picked up by name but holds no backdrop key",
            scene.name
        );
        let fresh = render::<VulkanHal>(&mut ctx, &without);

        // The same budget the catalog's own comparison uses, read the other
        // way round: a difference this small is what two rasterizers may
        // legitimately disagree by, so anything at or under it is not evidence
        // that the key did something.
        assert!(
            !accepts(
                &compare(&keyed, &fresh).expect("the two renders are the same size"),
                CATALOG
            ),
            "{} renders the same with its backdrop keys as without them, so it \
             cannot show what it is named for",
            scene.name
        );
        checked += 1;
    }
    assert_eq!(
        checked, 3,
        "three keyed plates share a capture between panels; this checked {checked}"
    );
}
