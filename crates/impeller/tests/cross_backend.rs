//! Canvas-level scenes rendered on every backend and compared.
//!
//! The scene corpus already diffs the two backends against each other, but it
//! is expressed at the batch level: one target, one list of items, no nesting.
//! Everything the canvas adds on top of that — layers and the passes they turn
//! into, targets smaller than the frame, glyph runs with their own quads and
//! texture coordinates — cannot be stated as a corpus scene at all, and so has
//! never been compared. Vulkan encodes a pass into a command buffer while GLES
//! rebinds a framebuffer on a global state machine; those are different enough
//! that agreeing on flat geometry says little about agreeing on a layer.
//!
//! Two things are checked here, and they fail differently. That a scene renders
//! the same on both backends is a claim about the HAL. That a layer given
//! bounds renders the same as one without is a claim about the canvas, and it
//! is checked per backend rather than only on the first, because a mapping that
//! is off by the target's origin would be invisible on a full-size layer and
//! could differ between the two.

use impeller::{
    Backend, BackendPreference, Canvas, Color, Context, Extent2D, GradientStop, Layer, Paint,
    PathBuilder, PixelFormat, Recording, Rect, Vec2,
};
use impeller_testkit::image::{accepts, compare, Image, Tolerance};

const SIZE: Extent2D = Extent2D {
    width: 128,
    height: 128,
};

/// Every backend this build can reach, named.
///
/// A build with one backend compiled in yields one, and the cross-backend
/// comparison below reports that it had nothing to compare rather than passing
/// on a single element.
fn backends() -> Vec<(Backend, Context)> {
    [BackendPreference::Vulkan, BackendPreference::Gles]
        .into_iter()
        .filter_map(|preference| match Context::new(preference) {
            Ok(ctx) => Some((ctx.backend(), ctx)),
            Err(e) => {
                eprintln!("{preference:?} unavailable: {e}");
                None
            }
        })
        .collect()
}

fn render(ctx: &mut Context, recording: &Recording) -> Image {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    Image::new(SIZE.width, SIZE.height, pixels)
}

/// The region the layered scene confines itself to.
const BOUNDS: Rect = Rect {
    left: 24.0,
    top: 40.0,
    right: 96.0,
    bottom: 104.0,
};

/// A scene built entirely out of what the corpus cannot say.
///
/// Layers nested two deep, one of them under a transform, with a scissor and a
/// stencil clip inside, and a gradient so that a paint's own mapping is
/// exercised alongside the geometry's. Every one of those derives from where
/// the target is, which is the thing `bounds` changes.
fn layered_scene(bounds: bool) -> Recording {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::rgba8(12, 14, 20, 255));

    let open = |canvas: &mut Canvas, layer: Layer, region: Rect| {
        if bounds {
            canvas.save_layer_bounds(layer, region);
        } else {
            canvas.save_layer(layer);
        }
    };

    open(&mut canvas, Layer::opacity(0.75), BOUNDS);
    canvas
        .draw_rect(
            BOUNDS,
            &Paint::linear_gradient(
                Vec2::new(BOUNDS.left, BOUNDS.top),
                Vec2::new(BOUNDS.right, BOUNDS.bottom),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.25, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.35, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    canvas.save();
    canvas
        .clip_rect(Rect::new(30.0, 46.0, 90.0, 98.0))
        .expect("scissor");
    let mut triangle = PathBuilder::new();
    triangle
        .move_to(Vec2::new(60.0, 44.0))
        .line_to(Vec2::new(92.0, 100.0))
        .line_to(Vec2::new(28.0, 100.0))
        .close();
    canvas.clip_path(&triangle.build()).expect("stencil");

    canvas.translate(4.0, 6.0);
    // The region contains the circle exactly. Stated in the space in force
    // here, which the translate has already moved, so a bound that ignored the
    // transform would fall short of the shape by the offset -- and the layer
    // would clip it, which is what it is for.
    open(
        &mut canvas,
        Layer::opacity(0.5),
        Rect::new(38.0, 52.0, 82.0, 96.0),
    );
    canvas
        .draw_circle(
            Vec2::new(60.0, 74.0),
            22.0,
            &Paint::fill(Color::linear(0.15, 1.0, 0.4, 1.0)).with_anti_alias(false),
        )
        .expect("circle");
    canvas.restore();

    canvas.restore();
    canvas.restore();
    canvas.finish()
}

#[test]
fn the_backends_agree_on_a_layered_scene() {
    let mut contexts = backends();
    if contexts.len() < 2 {
        eprintln!(
            "skipping: {} backend(s) available, so there is nothing to compare",
            contexts.len()
        );
        return;
    }

    // Bit-exact. Nothing here is antialiased and nothing samples with a filter
    // that a driver is free to differ on, so a difference of one unit would be
    // a real disagreement rather than rounding, and a budget would only hide
    // it. The corpus raises tolerance for scene classes that earn it; this is
    // not one of them.
    let tolerance = Tolerance {
        per_channel: 0,
        outlier_fraction: 0.0,
    };

    for bounds in [false, true] {
        let recording = layered_scene(bounds);
        let mut rendered: Vec<(Backend, Image)> = Vec::new();
        for (backend, ctx) in &mut contexts {
            rendered.push((*backend, render(ctx, &recording)));
        }

        let (first_backend, first) = &rendered[0];
        assert!(
            first.pixels.iter().any(|&b| b > 48),
            "the scene rendered nothing, so the comparison proves nothing"
        );
        for (backend, image) in &rendered[1..] {
            let difference = compare(first, image).expect("comparable");
            assert!(
                accepts(&difference, tolerance),
                "{first_backend} and {backend} disagree on the layered scene \
                 (bounds: {bounds}): {difference:?}"
            );
        }
    }
}

#[test]
fn every_backend_renders_a_bounded_layer_like_a_full_size_one() {
    let mut contexts = backends();
    assert!(
        !contexts.is_empty(),
        "no backend at all, so nothing below ran"
    );

    let bounded = layered_scene(true);
    assert!(
        bounded
            .passes
            .iter()
            .any(|p| p.extent.width < SIZE.width || p.extent.height < SIZE.height),
        "no pass got a smaller target, so this compares two identical paths"
    );
    let full = layered_scene(false);

    for (backend, ctx) in &mut contexts {
        let with = render(ctx, &bounded);
        let without = render(ctx, &full);
        let difference = compare(&without, &with).expect("comparable");
        // Bounds are an optimization: the same drawing, into a target that
        // happens to be smaller. Anything but an exact match means a mapping
        // disagreed about where that target is.
        assert_eq!(
            difference.max_delta, 0,
            "{backend} moved pixels when the layer was given bounds: {difference:?}"
        );
    }
}
