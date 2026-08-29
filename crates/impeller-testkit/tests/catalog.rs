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

/// The blue an unoverlapped `BLUE_HALF` stroke leaves on the catalog ground.
///
/// Read off a render rather than derived, and the two doubled values are
/// nowhere near it: a second cover reads 226 and a sixth reads 251, so a
/// threshold between them does not need to be exact to mean what it says.
const SINGLE_COVER_BLUE: u8 = 140;

#[test]
fn a_wide_stroke_through_its_own_call_covers_each_pixel_once() {
    // Upstream keeps two forms of this scene, one drawing the rectangle
    // directly and one handing over its path, and both are named for the
    // stroke not overlapping itself. The half alpha is what makes the claim
    // legible: an outline covering a pixel twice is invisible at full opacity
    // and darker at half.
    //
    // The two routes here do not agree, and that is recorded rather than
    // asserted -- `docs/non-parity.md` section 13, with these numbers. What is
    // asserted is the half that holds: where the stroke goes through the call
    // the public API offers for a rectangle, the middle of a rectangle whose
    // stroke is twice its width carries exactly one cover. That is a real
    // property and nothing else in the suite protects it.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let find = |name: &str| {
        catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"))
    };
    let direct = find("basic/can-render-wide-stroked-rect-without-overlap");
    let as_path = find("basic/can-render-wide-stroked-rect-path-without-overlap");
    assert_ne!(
        direct.items, as_path.items,
        "the pair is supposed to differ in how it says the rectangle"
    );

    // The round-join rectangle of the lower row, which is the one this
    // renderer draws analytically: a stroked rectangle with a square corner
    // falls back to the tessellator, so the other two columns are the same
    // picture in both plates and have nothing to say here.
    let img = render::<VulkanHal>(&mut ctx, &direct);
    let mut covered = 0usize;
    let mut worst = (0u8, (0u32, 0u32));
    for y in 68..98u32 {
        for x in 49..79u32 {
            let blue = img.pixel(x, y)[2];
            if blue > worst.0 {
                worst = (blue, (x, y));
            }
            if blue.abs_diff(SINGLE_COVER_BLUE) <= 2 {
                covered += 1;
            }
        }
    }
    assert!(
        worst.0 <= SINGLE_COVER_BLUE + 20,
        "a pixel at {:?} reads {} where one cover is {SINGLE_COVER_BLUE}, so the \
         outline is blending over itself",
        worst.1,
        worst.0
    );
    assert!(
        covered > 700,
        "only {covered} of 900 pixels carry a full cover, so this window is \
         edge rather than stroke and the bound above proved nothing"
    );
}

fn forget_mask_blurs(nodes: &mut [impeller_testkit::Node]) -> usize {
    let mut dropped = 0;
    for node in nodes {
        match node {
            impeller_testkit::Node::Draw(item) => {
                if item.mask_blur > 0.0 {
                    item.mask_blur = 0.0;
                    dropped += 1;
                }
            }
            impeller_testkit::Node::Layer { children, .. } => {
                dropped += forget_mask_blurs(children);
            }
            impeller_testkit::Node::Picture(spec) => {
                dropped += forget_mask_blurs(&mut spec.children);
            }
            _ => {}
        }
    }
    dropped
}

#[test]
fn every_plate_that_asks_for_a_mask_blur_can_show_one() {
    // The failure this is here for has happened in this catalog before, to the
    // backdrop-blur plates: two scenes rendered identically to themselves with
    // the filter taken away, so they asked for it and could not show it. A
    // plate that cannot tell whether the thing it is named for happened is
    // worse than no plate, because the inventory counts it as covered.
    //
    // Nothing else catches it. Cross-backend comparison says the two backends
    // agree, and they agree just as well on the wrong picture. So every plate
    // holding a mask blur is rendered again with the sigma set to zero and has
    // to come out different.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let mut checked = 0usize;
    for scene in catalog() {
        let mut without = scene.clone();
        if forget_mask_blurs(&mut without.items) == 0 {
            continue;
        }
        if !scene.supported_by(ctx.capabilities()) {
            eprintln!("skipping: {} is not supported here", scene.name);
            continue;
        }
        let blurred = render::<VulkanHal>(&mut ctx, &scene);
        let sharp = render::<VulkanHal>(&mut ctx, &without);
        assert!(
            !accepts(
                &compare(&blurred, &sharp).expect("the two renders are the same size"),
                CATALOG
            ),
            "{} renders the same with its mask blur as without it, so it cannot \
             show what it asked for",
            scene.name
        );
        checked += 1;
    }
    assert!(
        checked >= 25,
        "only {checked} plates were found to hold a mask blur, which is fewer \
         than the catalog has and means the walk missed some"
    );
}

#[test]
fn a_layers_blur_widens_with_the_transform_it_was_opened_under() {
    // A filter on a save layer is stated in the space of the caller, not in
    // device pixels, so the same layer opened under a threefold scale blurs
    // three times as wide on screen. Upstream draws its scene twice at two
    // scales to say so, and this repository's architecture document records
    // taking the same position deliberately -- the sigma is local and the
    // conversion happens where the layer opens.
    //
    // A renderer that treated the sigma as already-device would draw the two
    // panels with the same soft edge and differ only in size, which is a
    // difference nobody comparing them by eye would necessarily notice. The
    // edge is measured instead.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/save-layer-filters-scale-with-transform")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    // How many pixels the right edge of each panel takes to fall from covered
    // to ground, counted along one row well clear of the corners.
    let ramp = |y: u32, xs: std::ops::Range<u32>| {
        xs.filter(|x| (20..250).contains(&img.pixel(*x, y)[0]))
            .count()
    };
    let small = ramp(16, 14..40);
    let large = ramp(74, 90..128);
    assert!(
        small >= 3 && large >= 3,
        "neither panel should have a hard edge; they measured {small} and {large}"
    );
    let ratio = large as f32 / small as f32;
    assert!(
        (2.2..3.8).contains(&ratio),
        "the panel drawn at three times the scale spread its blur over {large} \
         pixels against the other's {small}, a ratio of {ratio:.2}; a sigma \
         carried in device pixels would put that at one"
    );
}

#[test]
fn an_empty_layer_composites_nothing_at_all() {
    // A layer with no contents still allocates a target and still composites
    // it, so what this asks is whether that target was cleared: a layer
    // composited over a buffer nobody wrote would show as a rectangle of
    // whatever the allocation held, exactly where the layer's bounds are.
    //
    // Upstream's scene paints the frame red, opens a layer with a blue paint
    // and closes it at once, and the frame stays red. Here the frame is one
    // color or the plate has failed, which is a thing a test can say exactly.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/empty-save-layer-ignores-paint")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);
    let ground = img.pixel(0, 0);
    for y in 0..128 {
        for x in 0..128 {
            assert_eq!(
                img.pixel(x, y),
                ground,
                "({x}, {y}) is not the color the rest of the frame is, so the \
                 empty layer left something behind"
            );
        }
    }
    assert_eq!(
        ground,
        [255, 0, 0, 255],
        "the frame should be the paint's red"
    );
}

#[test]
fn the_same_thin_line_said_four_ways_says_what_it_should() {
    // Upstream draws its thin-line grid four times -- as a line, as a stroked
    // path, as a filled rectangle and as a filled rounded one -- because a
    // renderer may specialize any of them and a specialization that disagrees
    // with the general route is a bug nobody sees until the two are side by
    // side. Two claims here, and neither is the same as "they all agree".
    //
    // The line and the path are the same picture to the byte, because
    // `Canvas::draw_line` builds a two-point path and hands it to
    // `draw_path`. That is worth pinning rather than assuming: if the line
    // ever gains a route of its own, this is what says so, and the plates are
    // already there to look at.
    //
    // The filled forms are not the same picture, and should not be. A stroke
    // narrower than a device pixel is widened to one and dimmed to match,
    // which is upstream's rule and is copied constant for constant; a filled
    // rectangle a third of a pixel tall gets a third of a pixel of coverage
    // and nothing else. So the middle column of the grid is where the two
    // families part, and this asserts that they do -- a renderer that had
    // dropped the thin-stroke rule would make all four agree and would look,
    // from any other test here, entirely well.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut get = |n: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == n)
            .unwrap_or_else(|| panic!("{n} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };

    let line = get("path/draw-lines-with-draw-line");
    let path = get("path/draw-lines-with-path");
    let diff = compare(&line, &path).expect("the two renders are the same size");
    assert_eq!(
        diff.differing, 0,
        "draw_line and a two-point stroked path should be the same picture \
         while the first is written in terms of the second: {diff:?}"
    );

    for name in [
        "path/draw-lines-with-filled-rects",
        "path/draw-lines-with-filled-round-rects",
    ] {
        let filled = get(name);
        let diff = compare(&line, &filled).expect("the two renders are the same size");
        assert!(
            !accepts(&diff, CATALOG),
            "{name} renders the same as the stroked line, so the widening a \
             sub-pixel stroke gets is not being applied: {diff:?}"
        );
    }
}

#[test]
fn a_tile_mode_family_draws_four_different_pictures() {
    // Eight plates say the same seven-stop ramp under the four tile modes,
    // linear and swept, and the whole of what they are for is the difference
    // between them: the ramp covers a third of the shape, so what fills the
    // rest is the tile mode and nothing else. A renderer that quietly treated
    // one mode as another -- decal as clamp is the easy mistake, since both
    // leave the ramp's end color at the boundary -- would draw two of these
    // identically and pass every other test here.
    //
    // Pairwise rather than against a reference, because there is no reference:
    // no mode is the "right" one to compare the others to, and what is being
    // asserted is that four distinct things happened.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    for family in ["linear-gradient-many-colors", "sweep-gradient-many-colors"] {
        let imgs: Vec<(&str, Image)> = ["clamp", "repeat", "mirror", "decal"]
            .into_iter()
            .map(|tile| {
                let name = format!("gradient/can-render-{family}-{tile}");
                let scene = catalog()
                    .into_iter()
                    .find(|s| s.name == name)
                    .unwrap_or_else(|| panic!("{name} is not in the catalog"));
                (tile, render::<VulkanHal>(&mut ctx, &scene))
            })
            .collect();
        for i in 0..imgs.len() {
            for j in i + 1..imgs.len() {
                let diff = compare(&imgs[i].1, &imgs[j].1).expect("same size");
                assert!(
                    !accepts(&diff, CATALOG),
                    "{family}: {} and {} are the same picture, so one of the two \
                     tile modes is not being applied: {diff:?}",
                    imgs[i].0,
                    imgs[j].0
                );
            }
        }
    }
}

#[test]
fn reversing_a_gradient_reverses_the_picture() {
    // Upstream keeps each axis-aligned gradient in both directions, and its
    // `VerifyNonOptimizedGradient` beside them with the endpoints pulled inside
    // the shape so that whatever fast route the others may take, that one must
    // not. The stops are bunched at one end -- nought, a tenth, and one -- so
    // the picture is lopsided and a reversal is not a symmetry.
    //
    // What this catches is a fast path keyed on the axis that forgot the
    // direction, which would draw the forward and reversed plates the same and
    // is exactly the kind of thing a per-draw optimization gets wrong.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut shot = |name: String| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };
    for axis in ["horizontal", "vertical"] {
        let forward = shot(format!("gradient/fast-gradient-test-{axis}"));
        let back = shot(format!("gradient/fast-gradient-test-{axis}-reversed"));
        let diff = compare(&forward, &back).expect("same size");
        assert!(
            !accepts(&diff, CATALOG),
            "the {axis} gradient draws the same forward and reversed, so the \
             direction is being dropped: {diff:?}"
        );
    }
    // And the one that must not take the fast route differs from the one that
    // may, which is what says the condition was tested rather than assumed.
    let plain = shot("gradient/fast-gradient-test-vertical".to_string());
    let inset = shot("gradient/verify-non-optimized-gradient".to_string());
    let diff = compare(&plain, &inset).expect("same size");
    assert!(
        !accepts(&diff, CATALOG),
        "a gradient inset within its shape and repeating draws the same as one \
         spanning it and clamping: {diff:?}"
    );
}

#[test]
fn a_stroke_wider_than_its_arc_stops_exactly_at_the_frontier() {
    // Upstream draws a line at the rectangle's right side plus half the stroke
    // width and leaves a reader to check that the fat white arc reaches it and
    // does not pass it. The plate keeps the line, and this does the checking.
    //
    // It is worth checking rather than looking at because the stroke is wider
    // than the shape's radius -- half of forty-eight against twenty -- so the
    // inner offset has crossed the center and the outline self-intersects.
    // That is where a stroker either clamps, and falls short, or runs away, and
    // the frontier is the only place the difference is a number.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    // Without the marker, which would otherwise cover the edge it marks.
    let mut scene = catalog()
        .into_iter()
        .find(|s| s.name == "path/fat-stroke-arc")
        .expect("the plate is in the catalog");
    let marker = scene.items.pop();
    assert!(
        marker.is_some(),
        "the plate should end with its frontier line"
    );
    let img = render::<VulkanHal>(&mut ctx, &scene);

    let white_in = |x: u32| (0..128u32).map(|y| img.pixel(x, y)[1]).max().unwrap_or(0);
    // The frontier is at eighty-four, so the last pixel the arc may touch is
    // the one spanning eighty-three to eighty-four, and it is touched
    // partially. Eighty-two is inside and has to be covered outright.
    assert_eq!(
        white_in(82),
        255,
        "the arc should reach a full cover one pixel inside the frontier"
    );
    assert!(
        white_in(83) > 0x11,
        "the arc should reach into the pixel the frontier passes through"
    );
    for x in 85..128 {
        assert_eq!(
            white_in(x),
            0x11,
            "the arc put white at x={x}, past a frontier at eighty-four"
        );
    }
}

#[test]
fn a_shape_clip_cuts_a_circle_through_its_center() {
    // The clip's corner is the circle's center, so what survives is one
    // quarter: two straight edges meeting where a curve used to be. A clip
    // that had been applied as the shape's bounding box would leave the whole
    // circle, and one applied a pixel out would show along two straight edges
    // forty pixels long, which is the easiest kind of difference to measure.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "path/can-render-clips")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);
    let ground = img.pixel(0, 0);
    let mut drawn = 0usize;
    for y in 0..128u32 {
        for x in 0..128u32 {
            if img.pixel(x, y) != ground {
                assert!(
                    x < 64 && y < 64,
                    "({x}, {y}) is outside a clip that ends at sixty-four"
                );
                drawn += 1;
            }
        }
    }
    // A quarter of a disc of radius forty-four is about fifteen hundred.
    assert!(
        (1400..1700).contains(&drawn),
        "a quarter of the circle should survive the clip; {drawn} pixels did"
    );
}

#[test]
fn an_identity_matrix_filter_changes_nothing_it_passes_through() {
    // Upstream draws the same filtered image twice, once with an identity
    // matrix image filter on it, and keeps them side by side for a reader to
    // see that they agree. It puts the filter there to take the draw off its
    // atlas fast path; there is no such path here, so what the pair says
    // instead is the claim that survives the translation: an identity matrix
    // filter routes the draw through an offscreen and resamples it on the way
    // back, and must come out the same anyway.
    //
    // That is not free of ways to fail. A half-texel offset in the resample, a
    // target sized to the wrong bounds, a color filter applied on the way in
    // rather than on the way out -- each would show here and in nothing else.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    for name in [
        "atlas/draw-image-rect-with-blend-color-filter",
        "atlas/draw-image-rect-with-matrix-color-filter",
    ] {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        let img = render::<VulkanHal>(&mut ctx, &scene);
        // The two panels are fifty-six wide, one at four and one at
        // sixty-eight, so the same offset into each is sixty-four apart.
        let mut differing = 0usize;
        let mut worst = 0u8;
        for y in 36..92u32 {
            for x in 4..60u32 {
                let (a, b) = (img.pixel(x, y), img.pixel(x + 64, y));
                let apart = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
                worst = worst.max(apart);
                if apart > 2 {
                    differing += 1;
                }
            }
        }
        assert!(
            differing * 100 < 56 * 56,
            "{name}: {differing} of {} pixels differ between the filtered draw \
             and the plain one, worst by {worst}",
            56 * 56
        );
    }
}

#[test]
fn a_blur_along_one_axis_leaves_the_other_alone() {
    // `dart:ui`'s `ImageFilter.blur` states `sigmaX` and `sigmaY` separately,
    // and a blur here is two separable passes already -- so the second
    // deviation is a field rather than a mechanism. What has to be checked is
    // that it is *used*: a renderer that took one sigma and applied it both
    // ways would draw a square halo where these draw a band, and a renderer
    // that skipped the wrong pass would draw the band the wrong way round.
    //
    // The pass whose deviation is zero is skipped rather than run as an
    // identity, which matters beyond the pass it saves: the reduction that
    // makes a wide blur affordable shrinks the image, so a zero-sigma pass
    // that still resampled would soften the axis it was supposed to leave
    // alone. That is checked here as the sharp axis staying the shape's own
    // width.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut extent = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        let img = render::<VulkanHal>(&mut ctx, &scene);
        let lit: Vec<(u32, u32)> = (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| img.pixel(*x, *y)[0] > 20)
            .collect();
        assert!(!lit.is_empty(), "{name} drew nothing");
        let x = lit.iter().map(|p| p.0).max().unwrap() - lit.iter().map(|p| p.0).min().unwrap();
        let y = lit.iter().map(|p| p.1).max().unwrap() - lit.iter().map(|p| p.1).min().unwrap();
        (x, y)
    };
    let across = extent("blur/a-blur-along-one-axis");
    let down = extent("blur/a-blur-along-the-other-axis");

    // The square is thirty-two on a side, so an unblurred axis measures about
    // thirty-one between its first and last lit pixel and a blurred one runs
    // well past it.
    assert!(
        across.1 <= 33,
        "the axis with no deviation spread to {} pixels from a square of \
         thirty-two, so the pass that should have been skipped ran",
        across.1
    );
    assert!(
        across.0 > across.1 + 12,
        "the blurred axis measured {} against the sharp one's {}, which is \
         not a band",
        across.0,
        across.1
    );
    // And the other plate is the same thing turned, which is what says the two
    // deviations reached the two passes rather than one reaching both.
    assert_eq!(
        (down.1, down.0),
        (across.0, across.1),
        "the two plates should be each other transposed"
    );
}

#[test]
fn composing_a_channel_swap_with_a_blur_gives_the_same_picture_either_way() {
    // Upstream keeps both orders, and they are the same picture on purpose: a
    // channel swap is a permutation matrix, a blur is a weighted sum, and two
    // linear operators commute. So the plates agree, and the agreement is the
    // assertion rather than a redundancy.
    //
    // What it catches is a composition that applied only the outer filter.
    // That failure keeps both plates valid-looking -- one would be a recolored
    // sharp circle and the other a blurred green one -- and it makes them
    // differ, which nothing else here would notice. Dropping the *inner* one
    // instead is caught by the rule that a scene carrying a filter has to
    // render differently without it.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut shot = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };
    let inner = shot("blur/compose-paint-blur-inner");
    let outer = shot("blur/compose-paint-blur-outer");
    let diff = compare(&inner, &outer).expect("the two renders are the same size");
    assert_eq!(
        diff.differing, 0,
        "a permutation and a blur commute, so composing them either way is one \
         operator; these differ, which means one of the two was dropped: {diff:?}"
    );
    // And the swap ran at all: green in, red out.
    let middle = inner.pixel(64, 64);
    assert!(
        middle[0] > middle[1],
        "the swap should take the circle's green to red; the middle reads {middle:?}"
    );
}

#[test]
fn a_clip_cuts_a_blurred_shape_after_the_blur_rather_than_before() {
    // The halo has to stop dead at the clip and the shape's own edge has to
    // stay soft. Blurring what the clip left would soften the cut as well, and
    // the two are told apart by looking at one edge of each in the same frame.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "blur/can-render-clipped-blur")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    // The clip's right edge is at a hundred and twelve, and the circle reaches
    // past it, so the transition there is one pixel wide.
    let green = |x: u32, y: u32| img.pixel(x, y)[1];
    assert!(
        green(110, 76) > 120 && green(114, 76) < 40,
        "the clip's edge should be a cut, and reads {} then {}",
        green(110, 76),
        green(114, 76)
    );
    // The circle's own top edge is nowhere near the clip and has to fade.
    let column: Vec<u8> = (28..48u32).map(|y| green(76, y)).collect();
    let partial = column.iter().filter(|g| (40..120).contains(*g)).count();
    assert!(
        partial >= 4,
        "the circle's unclipped edge should fade over several pixels; {partial} \
         of the column carried a partial value, so the blur is being cut off"
    );
}

#[test]
fn a_blurred_layer_survives_a_mirrored_transform() {
    // A mirrored transform has a negative determinant, so a renderer deriving
    // a layer's extent by transforming its corners and subtracting gets a
    // negative width -- a target of no size, which draws nothing at all. That
    // is the failure upstream keeps its flipped scene for, and it is loud:
    // either the layer is there or it is not.
    //
    // The content sits left of center in the layer's own space, so it has to
    // land right of center on the frame. Symmetric content would have made the
    // mirror invisible, and a renderer that dropped the flip would place the
    // rectangle on the wrong side while drawing an otherwise plausible
    // picture.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "blur/gaussian-blur-flipped")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    let red: Vec<(u32, u32)> = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let p = img.pixel(*x, *y);
            p[0] > 100 && p[1] < 100
        })
        .collect();
    assert!(
        red.len() > 1000,
        "the mirrored layer drew {} red pixels, which is not a layer",
        red.len()
    );
    let left = red.iter().map(|p| p.0).min().unwrap_or(0);
    let right = red.iter().map(|p| p.0).max().unwrap_or(0);
    assert!(
        left > 64,
        "content left of center in the layer's space belongs right of center \
         on the frame; it runs from {left} to {right}"
    );
    assert!(
        right <= 112,
        "and it must still stop at the layer's bounds, but reaches {right}"
    );
}

#[test]
fn a_clipped_blur_fills_its_window_and_turns_with_its_content() {
    // The clip is stated outside the transform, so the window stays put on the
    // frame while what is drawn into it moves: the blur's target is decided by
    // one space and its contents by another. Upstream keeps the scaled version
    // and the scaled-and-turned one side by side for that reason.
    //
    // Two things have to hold and they pull in opposite directions. The window
    // is filled edge to edge in both, so nothing about the transform may leave
    // a gap at the clip -- and the two are different pictures, so the rotation
    // has to reach the content. A renderer that clipped in the wrong space
    // would fail the first; one that dropped the rotation would fail the
    // second, while still drawing a plausible blurred window.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut shot = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };
    let flat = shot("blur/gaussian-blur-scaled-and-clipped");
    let turned = shot("blur/gaussian-blur-rotated-and-clipped");

    // The window is thirty-four to ninety-four and forty-five to eighty-three,
    // and every pixel of it has to differ from the ground outside.
    let ground = flat.pixel(0, 0);
    for img in [&flat, &turned] {
        let mut inside = 0usize;
        for y in 45..83u32 {
            for x in 34..94u32 {
                if img.pixel(x, y) != ground {
                    inside += 1;
                }
            }
        }
        assert_eq!(
            inside,
            60 * 38,
            "the clip's window should be filled edge to edge; {inside} of \
             {} pixels were",
            60 * 38
        );
        assert_eq!(img.pixel(33, 64), ground, "and nothing may fall outside it");
    }

    let diff = compare(&flat, &turned).expect("the two renders are the same size");
    assert!(
        !accepts(&diff, CATALOG),
        "the turned plate renders the same as the flat one, so the rotation is \
         not reaching the content: {diff:?}"
    );
}
