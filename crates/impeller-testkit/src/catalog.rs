//! Scenes mirroring the ones Impeller's own playground offers.
//!
//! Impeller's playground is a mode of its test suite: a `TEST_P` that calls
//! `OpenPlaygroundHere` renders a frame, and with the flag set it opens a
//! window instead of running headless. The scenes are therefore its tests,
//! grouped by topic across `aiks_dl_*_unittests.cc` -- basic, blend, blur,
//! clip, gradient, opacity, path, primitive shape, vertices, atlas, shadow,
//! text and runtime effects.
//!
//! This is the same set of pictures, as far as this renderer can draw them.
//! `docs/playground-parity.md` is the inventory: what each file contains, what
//! is reproduced here, and what is blocked and on which missing capability.
//!
//! # Why not the corpus
//!
//! The corpus is deliberately small and varied -- each scene is there because
//! it exercises something no other scene does, and its tolerances are tuned
//! per scene against a software reference. Several hundred pictures organized
//! around somebody else's test suite is a different thing with a different
//! purpose, and folding them together would ruin the property that makes the
//! corpus useful: that a failure in it names a capability rather than a
//! picture.
//!
//! So these are their own collection. They render on both backends and are
//! compared against each other, which is what makes them worth having as
//! tests rather than only as pictures, but they carry no per-scene tolerance
//! and no claim to be minimal.
//!
//! # Naming
//!
//! Every scene is named for the test it mirrors, in the form
//! `topic/CppTestName` reduced to kebab case. A reader who wants to know what
//! a scene is supposed to show can find the original by its name, and a
//! scene here with no counterpart there would be visible as one.

use crate::scene::{
    AtlasSpec, Fill, Item, LayerSpec, MeshSpec, Node, Scene, ShadowSpec, SpriteSpec, Stop,
    StrokeSpec, Transform,
};
use crate::shape::Shape;
use impeller_core::{Affine2, ImageFilter, MaskBlurStyle, Vec2, VertexMode};
use impeller_geometry::stroke::{LineCap, LineJoin};
use impeller_geometry::FillRule;
use impeller_hal::{BlendMode, ColorFilter, Sampling, TileMode};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.2, 0.4, 1.0, 1.0];
const YELLOW: [f32; 4] = [1.0, 0.9, 0.1, 1.0];
/// The ground every plate clears to.
///
/// Stated as exact eight-bit values rather than as round decimals, which is
/// not fussiness. The first version of this was `0.10` in blue, and `0.10`
/// times 255 is 25.5 -- a tie, which the two backends broke in opposite
/// directions. Every pixel of every plate then differed by one, the outlier
/// fraction went to ninety per cent, and the comparison that was supposed to
/// find a diverging backend was drowned by its own background.
const DARK: [f32; 4] = [15.0 / 255.0, 18.0 / 255.0, 26.0 / 255.0, 1.0];

/// Every catalog scene.
pub fn catalog() -> Vec<Scene> {
    let mut scenes = Vec::new();
    scenes.extend(basic());
    scenes.extend(path());
    scenes.extend(gradient());
    scenes.extend(clip());
    scenes.extend(opacity());
    scenes.extend(blend());
    scenes.extend(image());
    scenes.extend(vertices());
    scenes.extend(atlas_scenes());
    scenes.extend(blur());
    scenes.extend(shadow());
    scenes.extend(blur_variants());
    scenes.extend(layers());
    scenes.extend(runtime_effect());
    scenes.extend(image_filters());
    scenes
}

/// A scene on the catalog's own ground, which is dark so a white shape shows.
fn plate(name: &'static str, items: Vec<Item>) -> Scene {
    Scene::new(name, items)
        .with_background(DARK)
        .with_samples(4)
}

/// `aiks_dl_basic_unittests.cc` -- shapes, strokes and arcs.
fn basic() -> Vec<Scene> {
    vec![
        plate(
            "basic/can-render-colored-rect",
            vec![Item::fill(
                Shape::Rect {
                    min: [32.0, 32.0],
                    max: [96.0, 96.0],
                },
                BLUE,
            )],
        ),
        // Wide enough that a naive stroke would overlap itself at the corners,
        // which is what the original is checking for.
        plate(
            "basic/can-render-wide-stroked-rect-without-overlap",
            vec![Item::stroke(
                Shape::Rect {
                    min: [44.0, 44.0],
                    max: [84.0, 84.0],
                },
                StrokeSpec::new(28.0),
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-rects-render-correctly",
            vec![
                Item::stroke(
                    Shape::Rect {
                        min: [12.0, 12.0],
                        max: [60.0, 60.0],
                    },
                    StrokeSpec::new(4.0),
                    RED,
                ),
                Item::stroke(
                    Shape::Rect {
                        min: [68.0, 12.0],
                        max: [116.0, 60.0],
                    },
                    StrokeSpec::new(12.0),
                    GREEN,
                ),
                Item::stroke(
                    Shape::Rect {
                        min: [12.0, 68.0],
                        max: [60.0, 116.0],
                    },
                    StrokeSpec::new(1.0),
                    BLUE,
                ),
                Item::stroke(
                    Shape::Rect {
                        min: [68.0, 68.0],
                        max: [116.0, 116.0],
                    },
                    StrokeSpec {
                        join: LineJoin::Round,
                        ..StrokeSpec::new(10.0)
                    },
                    YELLOW,
                ),
            ],
        ),
        plate(
            "basic/filled-circles-render-correctly",
            vec![
                Item::fill(
                    Shape::Circle {
                        center: [40.0, 40.0],
                        radius: 28.0,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [88.0, 40.0],
                        radius: 16.0,
                    },
                    GREEN,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [40.0, 92.0],
                        radius: 8.0,
                    },
                    BLUE,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [92.0, 92.0],
                        radius: 2.0,
                    },
                    YELLOW,
                ),
            ],
        ),
        plate(
            "basic/draw-thin-stroked-circle",
            vec![Item::stroke(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 48.0,
                },
                StrokeSpec::new(0.5),
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-circles-render-correctly",
            vec![
                Item::stroke(
                    Shape::Circle {
                        center: [42.0, 42.0],
                        radius: 26.0,
                    },
                    StrokeSpec::new(2.0),
                    RED,
                ),
                Item::stroke(
                    Shape::Circle {
                        center: [90.0, 42.0],
                        radius: 22.0,
                    },
                    StrokeSpec::new(10.0),
                    GREEN,
                ),
                // Wider than the radius, so the band closes over the centre --
                // the case a stroke of a small circle has to get right.
                Item::stroke(
                    Shape::Circle {
                        center: [64.0, 96.0],
                        radius: 10.0,
                    },
                    StrokeSpec::new(24.0),
                    BLUE,
                ),
            ],
        ),
        plate(
            "basic/filled-ellipses-render-correctly",
            vec![
                Item::fill(
                    Shape::Oval {
                        min: [8.0, 24.0],
                        max: [120.0, 56.0],
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Oval {
                        min: [48.0, 68.0],
                        max: [80.0, 120.0],
                    },
                    BLUE,
                ),
            ],
        ),
        plate(
            "basic/filled-arcs-render-correctly",
            vec![
                Item::fill(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [52.0, 52.0],
                        start: -1.2,
                        sweep: 2.4,
                        through_center: true,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [34.0, 34.0],
                        start: 1.6,
                        sweep: 2.0,
                        through_center: true,
                    },
                    GREEN,
                ),
            ],
        ),
        plate(
            "basic/non-square-filled-arcs-render-correctly",
            vec![Item::fill(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [56.0, 30.0],
                    start: -0.6,
                    sweep: 4.0,
                    through_center: true,
                },
                YELLOW,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-round-ends",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [46.0, 46.0],
                    start: -2.2,
                    sweep: 3.6,
                    through_center: false,
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(14.0)
                },
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-butt-ends",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [46.0, 46.0],
                    start: -2.2,
                    sweep: 3.6,
                    through_center: false,
                },
                StrokeSpec::new(14.0),
                WHITE,
            )],
        ),
        plate(
            "basic/translucent-filled-arcs-render-correctly",
            vec![
                Item::fill(
                    Shape::Arc {
                        center: [56.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -1.0,
                        sweep: 2.6,
                        through_center: true,
                    },
                    [1.0, 0.2, 0.2, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Arc {
                        center: [76.0, 64.0],
                        radii: [44.0, 44.0],
                        start: 1.4,
                        sweep: 2.6,
                        through_center: true,
                    },
                    [0.2, 0.6, 1.0, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "basic/filled-round-rects-render-correctly",
            vec![
                Item::fill(
                    Shape::RoundedRect {
                        min: [10.0, 10.0],
                        max: [60.0, 50.0],
                        radius: 4.0,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::RoundedRect {
                        min: [68.0, 10.0],
                        max: [118.0, 50.0],
                        radius: 20.0,
                    },
                    GREEN,
                ),
                // Asked for more than half the shorter side, so it becomes a
                // stadium rather than an outline that crosses itself.
                Item::fill(
                    Shape::RoundedRect {
                        min: [10.0, 62.0],
                        max: [118.0, 118.0],
                        radius: 64.0,
                    },
                    BLUE,
                ),
            ],
        ),
        plate(
            "basic/can-render-rounded-rect-with-uniform-radii",
            vec![Item::stroke(
                Shape::RoundedRect {
                    min: [20.0, 32.0],
                    max: [108.0, 96.0],
                    radius: 16.0,
                },
                StrokeSpec::new(6.0),
                WHITE,
            )],
        ),
        plate(
            "basic/can-render-different-shapes-with-same-color-source",
            vec![
                Item::filled(
                    Shape::Circle {
                        center: [40.0, 40.0],
                        radius: 28.0,
                    },
                    ramp(),
                ),
                Item::filled(
                    Shape::RoundedRect {
                        min: [70.0, 12.0],
                        max: [120.0, 68.0],
                        radius: 12.0,
                    },
                    ramp(),
                ),
                Item::filled(
                    Shape::Polygon(vec![[20.0, 118.0], [64.0, 76.0], [108.0, 118.0]]),
                    ramp(),
                ),
            ],
        ),
        plate(
            "basic/compare-anti-alias-and-non-anti-alias",
            vec![
                Item::fill(
                    Shape::Circle {
                        center: [40.0, 64.0],
                        radius: 30.0,
                    },
                    WHITE,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [92.0, 64.0],
                        radius: 30.0,
                    },
                    WHITE,
                ),
            ],
        )
        .with_samples(1),
        plate(
            "basic/can-render-lines-with-caps-angles-and-alphas",
            (0..6)
                .map(|i| {
                    let t = i as f32 / 5.0;
                    let angle = t * std::f32::consts::PI;
                    let (dx, dy) = (angle.cos() * 44.0, angle.sin() * 44.0);
                    Item::stroke(
                        Shape::Polyline(vec![[64.0 - dx, 64.0 - dy], [64.0 + dx, 64.0 + dy]]),
                        StrokeSpec {
                            cap: match i % 3 {
                                0 => LineCap::Butt,
                                1 => LineCap::Round,
                                _ => LineCap::Square,
                            },
                            ..StrokeSpec::new(9.0)
                        },
                        [1.0, 1.0, 1.0, 0.3 + 0.7 * t],
                    )
                    .with_blend(BlendMode::SrcOver)
                })
                .collect(),
        ),
    ]
}

/// A gradient several scenes fill different shapes with, to show one color
/// source spanning shapes that have nothing else in common.
fn ramp() -> Fill {
    Fill::LinearGradient {
        start: [8.0, 8.0],
        end: [120.0, 120.0],
        stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
        tile: TileMode::Clamp,
    }
}

/// `aiks_dl_path_unittests.cc` -- curves, strokes and contours.
fn path() -> Vec<Scene> {
    vec![
        plate(
            "path/can-render-curved-strokes",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec::new(6.0),
                WHITE,
            )],
        ),
        plate(
            "path/can-render-thick-curved-strokes",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(28.0)
                },
                BLUE,
            )],
        ),
        plate(
            "path/can-render-thin-curved-strokes",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec::new(0.6),
                WHITE,
            )],
        ),
        plate(
            "path/can-render-stroke-path-that-ends-at-sharp-turn",
            vec![Item::stroke(
                Shape::Polyline(vec![[20.0, 108.0], [64.0, 20.0], [66.0, 108.0]]),
                StrokeSpec {
                    join: LineJoin::Miter,
                    ..StrokeSpec::new(12.0)
                },
                WHITE,
            )],
        ),
        plate(
            "path/can-draw-an-open-path",
            vec![Item::stroke(
                Shape::Polyline(vec![
                    [16.0, 96.0],
                    [48.0, 32.0],
                    [80.0, 96.0],
                    [112.0, 32.0],
                ]),
                StrokeSpec {
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    ..StrokeSpec::new(10.0)
                },
                GREEN,
            )],
        ),
        plate(
            "path/can-render-difference-paths",
            vec![Item::filled(
                Shape::RuledPolygon {
                    // Two concentric rings wound the same way, so the even-odd
                    // rule leaves the middle empty and a nonzero fill would
                    // not.
                    points: ring(48.0).into_iter().chain(ring(26.0)).collect(),
                    rule: FillRule::EvenOdd,
                },
                Fill::Solid(YELLOW),
            )],
        ),
        plate(
            "path/stroke-caps-and-joins",
            vec![
                Item::stroke(
                    Shape::Polyline(vec![[16.0, 24.0], [64.0, 24.0], [64.0, 52.0]]),
                    StrokeSpec {
                        cap: LineCap::Butt,
                        join: LineJoin::Miter,
                        ..StrokeSpec::new(12.0)
                    },
                    RED,
                ),
                Item::stroke(
                    Shape::Polyline(vec![[16.0, 68.0], [64.0, 68.0], [64.0, 96.0]]),
                    StrokeSpec {
                        cap: LineCap::Round,
                        join: LineJoin::Round,
                        ..StrokeSpec::new(12.0)
                    },
                    GREEN,
                ),
                Item::stroke(
                    Shape::Polyline(vec![[16.0, 112.0], [64.0, 112.0], [64.0, 124.0]]),
                    StrokeSpec {
                        cap: LineCap::Square,
                        join: LineJoin::Bevel,
                        ..StrokeSpec::new(12.0)
                    },
                    BLUE,
                ),
            ],
        ),
        plate(
            "path/can-draw-multi-contour-convex-path",
            vec![Item::fill(
                Shape::Polygon(vec![[16.0, 16.0], [56.0, 16.0], [56.0, 56.0], [16.0, 56.0]]),
                RED,
            )]
            .into_iter()
            .chain([Item::fill(
                Shape::Polygon(vec![
                    [72.0, 72.0],
                    [112.0, 72.0],
                    [112.0, 112.0],
                    [72.0, 112.0],
                ]),
                BLUE,
            )])
            .collect(),
        ),
        plate(
            "path/can-render-filled-conic-paths",
            vec![Item::fill(
                Shape::Conic {
                    start: [16.0, 104.0],
                    ctrl: [64.0, 8.0],
                    end: [112.0, 104.0],
                    weight: std::f32::consts::FRAC_1_SQRT_2,
                },
                BLUE,
            )],
        ),
        plate(
            "path/can-render-stroked-conic-paths",
            vec![Item::stroke(
                Shape::Conic {
                    start: [16.0, 104.0],
                    ctrl: [64.0, 8.0],
                    end: [112.0, 104.0],
                    weight: std::f32::consts::FRAC_1_SQRT_2,
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(8.0)
                },
                WHITE,
            )],
        ),
        plate(
            "path/can-render-tight-conic-path",
            vec![Item::stroke(
                // A weight well above one pulls the curve hard toward the
                // control point, which is where a subdivision that stops too
                // early shows a corner.
                Shape::Conic {
                    start: [24.0, 100.0],
                    ctrl: [64.0, 4.0],
                    end: [104.0, 100.0],
                    weight: 9.0,
                },
                StrokeSpec::new(5.0),
                YELLOW,
            )],
        ),
        plate(
            "path/solid-strokes-render-correctly",
            (0..5)
                .map(|i| {
                    let y = 20.0 + i as f32 * 22.0;
                    Item::stroke(
                        Shape::Polyline(vec![[14.0, y], [114.0, y]]),
                        StrokeSpec::new(1.0 + i as f32 * 3.5),
                        WHITE,
                    )
                })
                .collect(),
        ),
    ]
}

/// A regular polygon standing in for a circle, for the even-odd scene.
fn ring(radius: f32) -> Vec<[f32; 2]> {
    (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            [64.0 + radius * a.cos(), 64.0 + radius * a.sin()]
        })
        .collect()
}

/// `aiks_dl_gradient_unittests.cc` -- every gradient kind and tile mode.
fn gradient() -> Vec<Scene> {
    let band = Shape::Rect {
        min: [4.0, 4.0],
        max: [124.0, 124.0],
    };
    // A quarter of the way across, so every tile mode has something outside it
    // to act on -- which is the whole point of the four scenes below.
    let quarter = |tile| Fill::LinearGradient {
        start: [34.0, 0.0],
        end: [64.0, 0.0],
        stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
        tile,
    };
    let many = |count: usize| -> Vec<Stop> {
        (0..count)
            .map(|i| {
                let t = i as f32 / (count - 1) as f32;
                Stop::new([t, 0.4 + 0.5 * (1.0 - t), 1.0 - t, 1.0], t)
            })
            .collect()
    };

    vec![
        plate(
            "gradient/can-render-linear-gradient-clamp",
            vec![Item::filled(band.clone(), quarter(TileMode::Clamp))],
        ),
        plate(
            "gradient/can-render-linear-gradient-repeat",
            vec![Item::filled(band.clone(), quarter(TileMode::Repeat))],
        ),
        plate(
            "gradient/can-render-linear-gradient-mirror",
            vec![Item::filled(band.clone(), quarter(TileMode::Mirror))],
        ),
        plate(
            "gradient/can-render-linear-gradient-decal",
            vec![Item::filled(band.clone(), quarter(TileMode::Decal))],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-clamp",
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    stops: many(9),
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-way-many-colors-clamp",
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    stops: many(24),
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-uneven-stops",
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    // Bunched at one end, so the ramp is not a uniform sweep
                    // and a table built as though it were would be visibly
                    // wrong.
                    stops: vec![
                        Stop::new(RED, 0.0),
                        Stop::new(YELLOW, 0.05),
                        Stop::new(GREEN, 0.1),
                        Stop::new(BLUE, 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-with-incomplete-stops",
            vec![Item::filled(
                band.clone(),
                // Neither end of the unit interval is named, so the shader has
                // to hold the first and last colours beyond them.
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    stops: vec![Stop::new(RED, 0.3), Stop::new(BLUE, 0.7)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-radial-gradient",
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-radial-gradient-many-colors",
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: many(9),
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-clamp",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-repeat",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Repeat,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-mirror",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Mirror,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-decal",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Decal,
                },
            )],
        ),
        plate(
            "gradient/can-render-conical-gradient",
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [44.0, 44.0],
                    start_radius: 0.0,
                    end_center: [64.0, 64.0],
                    end_radius: 58.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-gradient-decal-with-background",
            vec![
                Item::fill(band.clone(), [0.25, 0.25, 0.3, 1.0]),
                Item::filled(band.clone(), quarter(TileMode::Decal)).with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "gradient/gradient-strokes-render-correctly",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 42.0,
                },
                Fill::LinearGradient {
                    start: [22.0, 0.0],
                    end: [106.0, 0.0],
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )
            .with_stroke(StrokeSpec::new(12.0))],
        ),
    ]
}

/// `aiks_dl_clip_unittests.cc`.
fn clip() -> Vec<Scene> {
    vec![
        plate(
            "clip/can-render-nested-clips",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                YELLOW,
            )
            // Two clips on one item, one axis-aligned and one not, so both
            // paths through the clip machinery narrow the same draw.
            .with_clip([20.0, 20.0, 108.0, 108.0])
            .with_clip_shape(Shape::Circle {
                center: [64.0, 64.0],
                radius: 40.0,
            })],
        ),
        plate(
            "clip/clips-use-current-transform",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                GREEN,
            )
            .with_clip_shape(Shape::Rect {
                min: [40.0, 40.0],
                max: [88.0, 88.0],
            })
            // The clip is stated in the item's own space, so it turns with it
            // -- a clip that did not would stay square here.
            .with_transform(Transform {
                rotate: 0.5,
                translate: [10.0, -6.0],
                ..Transform::default()
            })],
        ),
        plate(
            "clip/can-render-with-contiguous-clip-restores",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    RED,
                )
                .with_clip_shape(Shape::Circle {
                    center: [44.0, 64.0],
                    radius: 34.0,
                }),
                // A second item with its own clip: each is built and stepped
                // back independently, so the first one's clip must not still
                // be in force here.
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    BLUE,
                )
                .with_blend(BlendMode::SrcOver)
                .with_clip_shape(Shape::Circle {
                    center: [84.0, 64.0],
                    radius: 34.0,
                }),
            ],
        ),
    ]
}

/// `aiks_dl_opacity_unittests.cc`.
fn opacity() -> Vec<Scene> {
    let overlapping = || {
        vec![
            Node::Draw(Box::new(Item::fill(
                Shape::Circle {
                    center: [50.0, 64.0],
                    radius: 32.0,
                },
                RED,
            ))),
            Node::Draw(Box::new(
                Item::fill(
                    Shape::Circle {
                        center: [82.0, 64.0],
                        radius: 32.0,
                    },
                    BLUE,
                )
                .with_blend(BlendMode::SrcOver),
            )),
        ]
    };
    vec![
        Scene::tree(
            "opacity/can-render-group-opacity",
            vec![Node::Layer {
                // The picture that distinguishes a group's opacity from each
                // member's: where the two circles overlap, a group at half
                // alpha shows one blend and two half-alpha circles show two.
                layer: LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                },
                bounds: None,
                transform: Transform::default(),
                children: overlapping(),
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "opacity/draw-opacity-peephole",
            vec![Node::Layer {
                layer: LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                },
                bounds: Some([12.0, 24.0, 116.0, 104.0]),
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(Item::fill(
                    Shape::RoundedRect {
                        min: [20.0, 32.0],
                        max: [108.0, 96.0],
                        radius: 14.0,
                    },
                    WHITE,
                )))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

/// Every blend mode, by the name the catalog gives its scene.
///
/// Named here rather than derived from a `Display` implementation because a
/// scene's name is a `&'static str` and has to be one: it is an identifier the
/// playground steps through and a test reports, not a label. Writing them out
/// also means adding a mode to the renderer does not silently add a scene
/// nobody looked at.
const MODES: &[(BlendMode, &str)] = &[
    (BlendMode::Clear, "blend/blend-mode-clear"),
    (BlendMode::Src, "blend/blend-mode-src"),
    (BlendMode::Dst, "blend/blend-mode-dst"),
    (BlendMode::SrcOver, "blend/blend-mode-src-over"),
    (BlendMode::DstOver, "blend/blend-mode-dst-over"),
    (BlendMode::SrcIn, "blend/blend-mode-src-in"),
    (BlendMode::DstIn, "blend/blend-mode-dst-in"),
    (BlendMode::SrcOut, "blend/blend-mode-src-out"),
    (BlendMode::DstOut, "blend/blend-mode-dst-out"),
    (BlendMode::SrcATop, "blend/blend-mode-src-atop"),
    (BlendMode::DstATop, "blend/blend-mode-dst-atop"),
    (BlendMode::Xor, "blend/blend-mode-xor"),
    (BlendMode::Plus, "blend/blend-mode-plus"),
    (BlendMode::Modulate, "blend/blend-mode-modulate"),
    (BlendMode::Multiply, "blend/blend-mode-multiply"),
    (BlendMode::Screen, "blend/blend-mode-screen"),
    (BlendMode::Overlay, "blend/blend-mode-overlay"),
    (BlendMode::Darken, "blend/blend-mode-darken"),
    (BlendMode::Lighten, "blend/blend-mode-lighten"),
    (BlendMode::ColorDodge, "blend/blend-mode-color-dodge"),
    (BlendMode::ColorBurn, "blend/blend-mode-color-burn"),
    (BlendMode::HardLight, "blend/blend-mode-hard-light"),
    (BlendMode::SoftLight, "blend/blend-mode-soft-light"),
    (BlendMode::Difference, "blend/blend-mode-difference"),
    (BlendMode::Exclusion, "blend/blend-mode-exclusion"),
    (BlendMode::Hue, "blend/blend-mode-hue"),
    (BlendMode::Saturation, "blend/blend-mode-saturation"),
    (BlendMode::Color, "blend/blend-mode-color"),
    (BlendMode::Luminosity, "blend/blend-mode-luminosity"),
];

/// `aiks_dl_blend_unittests.cc`.
///
/// The per-mode scenes mirror what the original generates with a macro over
/// every blend mode. A scene naming an advanced mode declares that need by
/// containing one, so a device without the extension reports it as a gap
/// rather than as a difference.
fn blend() -> Vec<Scene> {
    let mut scenes: Vec<Scene> = MODES
        .iter()
        .map(|(mode, name)| {
            plate(
                name,
                vec![
                    // A destination with structure rather than a flat colour:
                    // dodge, burn and the two contrast modes are functions of
                    // what is underneath, and a flat backdrop would exercise
                    // one point of each curve.
                    Item::filled(
                        Shape::Rect {
                            min: [8.0, 8.0],
                            max: [120.0, 120.0],
                        },
                        Fill::LinearGradient {
                            start: [8.0, 8.0],
                            end: [120.0, 120.0],
                            stops: vec![
                                Stop::new([0.1, 0.1, 0.1, 1.0], 0.0),
                                Stop::new([0.95, 0.95, 0.95, 1.0], 1.0),
                            ],
                            tile: TileMode::Clamp,
                        },
                    ),
                    // Translucent, so the modes that read the source's alpha
                    // differ from the ones that do not.
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 40.0,
                        },
                        [0.9, 0.35, 0.2, 0.75],
                    )
                    .with_blend(*mode),
                ],
            )
        })
        .collect();

    scenes.push(plate(
        "blend/blend-mode-should-cover-whole-screen",
        vec![
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                RED,
            ),
            // A source covering everything, which is what the original checks:
            // a blend that left an unwritten margin would show the ground.
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                BLUE,
            )
            .with_blend(BlendMode::Screen),
        ],
    ));

    scenes.push(plate(
        "blend/color-filter-blend",
        vec![Item::filled(
            Shape::Rect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
            },
            ramp(),
        )
        // A blend against a constant, applied to the paint rather than to the
        // framebuffer -- which is the distinction this scene exists to show,
        // since the picture is the one the blend mode would give against a
        // flat destination of that colour.
        .with_color_filter(
            ColorFilter::blend([0.2, 0.5, 1.0, 1.0], BlendMode::SrcIn).expect("affine"),
        )],
    ));

    scenes.push(plate(
        "blend/color-filter-matrix",
        vec![Item::filled(
            Shape::Rect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
            },
            ramp(),
        )
        .with_color_filter(ColorFilter::matrix([
            0.2126, 0.7152, 0.0722, 0.0, 0.0, //
            0.2126, 0.7152, 0.0722, 0.0, 0.0, //
            0.2126, 0.7152, 0.0722, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, 0.0,
        ]))],
    ));

    scenes.push(plate(
        "blend/clear-blend",
        vec![
            Item::fill(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                GREEN,
            ),
            // Clear takes neither operand, so this punches a hole rather than
            // drawing anything -- and the ground showing through it is the
            // whole picture.
            Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 34.0,
                },
                WHITE,
            )
            .with_blend(BlendMode::Clear),
        ],
    ));

    scenes.push(plate(
        "blend/color-wheel",
        (0..12)
            .map(|i| {
                let turn = i as f32 / 12.0 * std::f32::consts::TAU;
                let at = [64.0 + 26.0 * turn.cos(), 64.0 + 26.0 * turn.sin()];
                Item::fill(
                    Shape::Circle {
                        center: at,
                        radius: 26.0,
                    },
                    [
                        0.5 + 0.5 * turn.cos(),
                        0.5 + 0.5 * (turn + 2.094).cos(),
                        0.5 + 0.5 * (turn + 4.189).cos(),
                        1.0,
                    ],
                )
                .with_blend(BlendMode::Plus)
            })
            .collect(),
    ));

    scenes
}

/// The whole sheet, in the destination it is usually drawn into.
const SHEET: [f32; 4] = [16.0, 16.0, 112.0, 112.0];
/// All of it.
const ALL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn sheet(rect: [f32; 4], source: [f32; 4], tile: TileMode, sampling: Sampling) -> Fill {
    Fill::Image {
        rect,
        source,
        tile,
        sampling,
        alpha: 1.0,
        tint: WHITE,
    }
}

/// The texture scenes from `aiks_dl_basic_unittests.cc`.
///
/// Kept apart from the rest of that file's plates only because they are the
/// ones that need the fixture, which is worth being able to see in one place.
fn image() -> Vec<Scene> {
    let whole = Shape::Rect {
        min: [0.0, 0.0],
        max: [128.0, 128.0],
    };
    // A destination smaller than the shape it fills, so every tile mode has
    // something outside it to act on.
    let quarter: [f32; 4] = [40.0, 40.0, 88.0, 88.0];

    vec![
        plate(
            "basic/can-render-image",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-clamp",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Clamp, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-repeat",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Repeat, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-mirror",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Mirror, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-decal",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Decal, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-clamp-with-translate",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Clamp, Sampling::Linear),
            )
            // The mapping travels with the item, so the clamped edges move
            // with it rather than staying where the rectangle was written.
            .with_transform(Transform {
                translate: [18.0, -12.0],
                ..Transform::default()
            })],
        ),
        plate(
            "basic/can-render-image-rect",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                // One quadrant of the sheet across the whole destination,
                // which is what a sprite out of a sheet is.
                sheet(
                    SHEET,
                    [0.0, 0.0, 0.5, 0.5],
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )],
        ),
        plate(
            "basic/draw-image-rect-src-outside-bounds",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                // A source running past the sheet's own edge, which clamping
                // has to answer without reading anything that is not there.
                sheet(
                    SHEET,
                    [0.5, 0.5, 1.6, 1.6],
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )],
        ),
        plate(
            "basic/can-render-inverted-image-with-color-filter",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
            )
            .with_color_filter(ColorFilter::matrix([
                -1.0, 0.0, 0.0, 0.0, 1.0, //
                0.0, -1.0, 0.0, 0.0, 1.0, //
                0.0, 0.0, -1.0, 0.0, 1.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]))],
        ),
        plate(
            "basic/image-color-source-effect-transform",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 52.0,
                },
                sheet(SHEET, ALL, TileMode::Repeat, Sampling::Linear),
            )
            .with_transform(Transform {
                rotate: 0.6,
                scale: [1.3, 0.8],
                ..Transform::default()
            })],
        ),
    ]
}

/// A triangle, as three corners of the plate.
fn triangle() -> Vec<[f32; 2]> {
    vec![[64.0, 16.0], [116.0, 108.0], [12.0, 108.0]]
}

fn mesh(name: &'static str, spec: MeshSpec) -> Scene {
    Scene::tree(name, vec![Node::Mesh(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

fn mesh_of(positions: Vec<[f32; 2]>, fill: Fill) -> MeshSpec {
    MeshSpec {
        mode: VertexMode::Triangles,
        positions,
        colors: Vec::new(),
        texture_coords: Vec::new(),
        indices: Vec::new(),
        fill,
        blend: BlendMode::SrcOver,
        transform: Transform::default(),
    }
}

/// `aiks_dl_vertices_unittests.cc`.
fn vertices() -> Vec<Scene> {
    let quad = vec![[16.0, 16.0], [112.0, 16.0], [112.0, 112.0], [16.0, 112.0]];
    let corners = vec![
        ALL_CORNERS[0],
        ALL_CORNERS[1],
        ALL_CORNERS[2],
        ALL_CORNERS[3],
    ];

    vec![
        mesh(
            "vertices/draw-vertices-solid-color-triangles-without-indices",
            mesh_of(triangle(), Fill::Solid(BLUE)),
        ),
        mesh(
            "vertices/draw-vertices-solid-color-triangles-with-indices",
            MeshSpec {
                positions: quad.clone(),
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(Vec::new(), Fill::Solid(GREEN))
            },
        ),
        mesh(
            "vertices/can-convert-triangle-fan-to-triangles",
            MeshSpec {
                mode: VertexMode::TriangleFan,
                positions: fan(),
                ..mesh_of(Vec::new(), Fill::Solid(YELLOW))
            },
        ),
        mesh(
            "vertices/draw-vertices-linear-gradient-without-indices",
            mesh_of(triangle(), ramp()),
        ),
        mesh(
            "vertices/vertices-geometry-color-uv-position-data",
            MeshSpec {
                positions: quad.clone(),
                // A colour at each corner, which no gradient describes: the
                // four are independent and the interior is all of them at once.
                colors: corners.clone(),
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(Vec::new(), Fill::Solid(WHITE))
            },
        ),
        mesh(
            "vertices/draw-vertices-premultiplies-colors",
            MeshSpec {
                positions: quad.clone(),
                // Alpha varying between the corners, which is the case that
                // tells a premultiplied interpolation from a straight one: the
                // transparent end fades toward nothing rather than staying
                // bright and only thinning.
                colors: vec![
                    [1.0, 0.3, 0.1, 1.0],
                    [1.0, 0.3, 0.1, 0.0],
                    [1.0, 0.3, 0.1, 0.0],
                    [1.0, 0.3, 0.1, 1.0],
                ],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(Vec::new(), Fill::Solid(WHITE))
            },
        ),
        mesh(
            "vertices/vertices-geometry-uv-position-data",
            MeshSpec {
                positions: quad.clone(),
                texture_coords: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/draw-vertices-image-source-with-texture-coordinates",
            MeshSpec {
                positions: quad.clone(),
                // Mirrored coordinates, so the sheet lands the other way round
                // from where the geometry sits -- which is what says the
                // coordinates were read rather than the position.
                texture_coords: vec![[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/draw-vertices-image-source-with-texture-coordinates-and-color-blending",
            MeshSpec {
                positions: quad,
                texture_coords: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                colors: corners,
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/vertices-geometry-uv-position-data-with-translate",
            MeshSpec {
                positions: triangle(),
                texture_coords: vec![[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                transform: Transform {
                    translate: [-18.0, 10.0],
                    ..Transform::default()
                },
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
    ]
}

/// Four colours, one per corner.
const ALL_CORNERS: [[f32; 4]; 4] = [
    [1.0, 0.2, 0.2, 1.0],
    [0.2, 1.0, 0.3, 1.0],
    [0.3, 0.4, 1.0, 1.0],
    [1.0, 0.9, 0.2, 1.0],
];

/// A fan around the plate's centre.
fn fan() -> Vec<[f32; 2]> {
    let mut points = vec![[64.0, 64.0]];
    for i in 0..=8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        points.push([64.0 + 48.0 * a.cos(), 64.0 + 48.0 * a.sin()]);
    }
    points
}

fn atlas(name: &'static str, spec: AtlasSpec) -> Scene {
    Scene::tree(name, vec![Node::Atlas(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

/// One sprite per quadrant of the sheet, laid out in a row.
fn quadrants(color: [f32; 4]) -> Vec<SpriteSpec> {
    (0..4)
        .map(|i| {
            let (sx, sy) = ((i % 2) as f32 * 4.0, (i / 2) as f32 * 4.0);
            SpriteSpec {
                source: [sx, sy, sx + 4.0, sy + 4.0],
                rotate: 0.0,
                scale: 6.0,
                translate: [10.0 + i as f32 * 28.0, 52.0],
                color,
            }
        })
        .collect()
}

/// `aiks_dl_atlas_unittests.cc`.
fn atlas_scenes() -> Vec<Scene> {
    vec![
        atlas(
            "atlas/draw-atlas-no-color",
            AtlasSpec {
                sprites: quadrants(WHITE),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-with-color-simple",
            AtlasSpec {
                sprites: (0..4)
                    .map(|i| SpriteSpec {
                        color: ALL_CORNERS[i],
                        ..quadrants(WHITE)[i]
                    })
                    .collect(),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-with-opacity",
            AtlasSpec {
                sprites: quadrants(WHITE),
                blend: BlendMode::SrcOver,
                alpha: 0.4,
            },
        ),
        atlas(
            "atlas/draw-atlas-no-color-full-size",
            AtlasSpec {
                sprites: vec![SpriteSpec {
                    source: [0.0, 0.0, 8.0, 8.0],
                    rotate: 0.0,
                    scale: 12.0,
                    translate: [16.0, 16.0],
                    color: WHITE,
                }],
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-advanced-and-transform",
            AtlasSpec {
                sprites: (0..6)
                    .map(|i| {
                        let t = i as f32 / 6.0;
                        let turn = t * std::f32::consts::TAU;
                        SpriteSpec {
                            source: [0.0, 0.0, 4.0, 4.0],
                            rotate: turn,
                            scale: 5.0,
                            translate: [
                                64.0 + 34.0 * turn.cos() - 10.0,
                                64.0 + 34.0 * turn.sin() - 10.0,
                            ],
                            color: WHITE,
                        }
                    })
                    .collect(),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
    ]
}

/// `aiks_dl_blur_unittests.cc`, as far as the styles reach.
///
/// The file is the largest of them and most of it turns on mask blur styles,
/// which is what these four are. What is still missing from it needs blurs
/// this renderer does not have -- an image filter on a backdrop identified by
/// a key, a blur that survives a rotation and a clip together, and the
/// tiny-mipmap cases.
fn blur() -> Vec<Scene> {
    let styles = [
        ("blur/gaussian-blur-style-normal", MaskBlurStyle::Normal),
        ("blur/gaussian-blur-style-solid", MaskBlurStyle::Solid),
        ("blur/gaussian-blur-style-outer", MaskBlurStyle::Outer),
        ("blur/gaussian-blur-style-inner", MaskBlurStyle::Inner),
    ];
    let mut scenes: Vec<Scene> = styles
        .iter()
        .map(|(name, style)| {
            plate(
                name,
                vec![Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 34.0,
                    },
                    WHITE,
                )
                .with_mask_blur(7.0)
                .with_mask_blur_style(*style)],
            )
        })
        .collect();

    scenes.push(plate(
        "blur/solid-color-circle-mask-blur-tiny-sigma",
        vec![Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 40.0,
            },
            WHITE,
        )
        // Small enough that the halo is a pixel or two, which is the case a
        // blur implemented by scaling a target down and back up gets wrong.
        .with_mask_blur(0.6)],
    ));

    scenes.push(plate(
        "blur/can-render-mask-blur-huge-sigma",
        vec![Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 20.0,
            },
            WHITE,
        )
        // Reaching well past the plate, so the halo is cut by the frame rather
        // than by the layer -- which is the distinction a bounded layer sized
        // to its content alone gets wrong.
        .with_mask_blur(30.0)],
    ));

    scenes.push(plate(
        "blur/mask-blur-with-zero-sigma-is-skipped",
        vec![Item::fill(
            Shape::RoundedRect {
                min: [24.0, 32.0],
                max: [104.0, 96.0],
                radius: 12.0,
            },
            WHITE,
        )
        .with_mask_blur(0.0)],
    ));

    scenes
}

fn shadow_plate(name: &'static str, spec: ShadowSpec) -> Scene {
    // A pale ground, because a shadow is a darkening and the plates elsewhere
    // are drawn on a dark one where it would be invisible.
    Scene::tree(name, vec![Node::Shadow(Box::new(spec))])
        .with_background([230.0 / 255.0, 230.0 / 255.0, 235.0 / 255.0, 1.0])
        .with_samples(4)
}

fn caster(shape: Shape) -> ShadowSpec {
    ShadowSpec {
        shape,
        color: [0.0, 0.0, 0.0, 1.0],
        elevation: 8.0,
        transparent_occluder: false,
        transform: Transform::default(),
        with_caster: true,
    }
}

/// `aiks_dl_shadow_unittests.cc`.
///
/// Most of that file checks an optimization for convex shadows -- one scene
/// per winding and shape kind, asserting the fast path was taken. This
/// renderer has no such optimization and the pictures are the same either way,
/// so what comes across is the shapes rather than the pairs.
fn shadow() -> Vec<Scene> {
    vec![
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-rect",
            caster(Shape::Rect {
                min: [36.0, 36.0],
                max: [92.0, 84.0],
            }),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-circle",
            caster(Shape::Circle {
                center: [64.0, 60.0],
                radius: 30.0,
            }),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-uniform-round-rect",
            caster(Shape::RoundedRect {
                min: [32.0, 36.0],
                max: [96.0, 84.0],
                radius: 14.0,
            }),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-oval",
            caster(Shape::Oval {
                min: [26.0, 42.0],
                max: [102.0, 80.0],
            }),
        ),
        shadow_plate(
            "shadow/can-draw-rotated-convex-shadow",
            ShadowSpec {
                transform: Transform {
                    rotate: 0.45,
                    translate: [14.0, -18.0],
                    ..Transform::default()
                },
                ..caster(Shape::Rect {
                    min: [36.0, 36.0],
                    max: [92.0, 84.0],
                })
            },
        ),
        shadow_plate(
            "shadow/can-draw-nonuniform-scale-convex-shadow",
            ShadowSpec {
                transform: Transform {
                    scale: [1.4, 0.7],
                    translate: [-26.0, 26.0],
                    ..Transform::default()
                },
                ..caster(Shape::RoundedRect {
                    min: [32.0, 36.0],
                    max: [96.0, 84.0],
                    radius: 12.0,
                })
            },
        ),
        shadow_plate(
            "shadow/transparent-shadow-produces-correct-color",
            ShadowSpec {
                // Nothing drawn on top, and the part beneath the caster kept:
                // the two together are what the flag is for, and the picture
                // is the whole blurred shape rather than a ring.
                transparent_occluder: true,
                with_caster: false,
                color: [0.1, 0.2, 0.6, 1.0],
                elevation: 10.0,
                ..caster(Shape::Circle {
                    center: [64.0, 60.0],
                    radius: 30.0,
                })
            },
        ),
    ]
}

/// Overlapping shapes, for the scenes that group things.
fn pair() -> Vec<Node> {
    vec![
        Node::Draw(Box::new(Item::fill(
            Shape::Circle {
                center: [50.0, 64.0],
                radius: 30.0,
            },
            RED,
        ))),
        Node::Draw(Box::new(
            Item::fill(
                Shape::Circle {
                    center: [82.0, 64.0],
                    radius: 30.0,
                },
                BLUE,
            )
            .with_blend(BlendMode::SrcOver),
        )),
    ]
}

fn grouped(name: &'static str, layer: LayerSpec, bounds: Option<[f32; 4]>) -> Scene {
    Scene::tree(
        name,
        vec![Node::Layer {
            layer,
            bounds,
            transform: Transform::default(),
            children: pair(),
        }],
    )
    .with_background(DARK)
    .with_samples(4)
}

/// The rest of `aiks_dl_blur_unittests.cc` that needs no capability this
/// renderer lacks.
///
/// The file's mask-blur variants are the same shape drawn at each style
/// against translucent and opaque colours, which is precisely what the styles
/// were built for. What is still missing from it wants a mask blur over a
/// gradient -- refused here, because blurring coverage and then filling is a
/// different picture from blurring the result unless the fill is constant --
/// or backdrop filters identified by a key across layers.
fn blur_variants() -> Vec<Scene> {
    let disc = |color: [f32; 4]| {
        Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 32.0,
            },
            color,
        )
        .with_blend(BlendMode::SrcOver)
    };
    let translucent = [1.0, 0.4, 0.2, 0.5];
    let opaque = [1.0, 0.4, 0.2, 1.0];

    let mut scenes = vec![
        plate(
            "blur/mask-blur-variant-test-normal-translucent",
            vec![disc(translucent).with_mask_blur(6.0)],
        ),
        plate(
            "blur/mask-blur-variant-test-normal-translucent-zero-sigma",
            vec![disc(translucent).with_mask_blur(0.0)],
        ),
        plate(
            "blur/mask-blur-variant-test-solid-translucent",
            vec![disc(translucent)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Solid)],
        ),
        plate(
            "blur/mask-blur-variant-test-solid-opaque",
            vec![disc(opaque)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Solid)],
        ),
        plate(
            "blur/mask-blur-variant-test-inner-translucent",
            vec![disc(translucent)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Inner)],
        ),
        plate(
            "blur/mask-blur-variant-test-outer-translucent",
            vec![disc(translucent)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Outer)],
        ),
        plate(
            "blur/blur-has-no-edge",
            vec![
                // Larger than the plate, so the halo is cut by the frame. A
                // bounded layer sized to its content alone puts a straight
                // edge where the blur should simply continue past the view.
                Item::fill(
                    Shape::Rect {
                        min: [-40.0, 40.0],
                        max: [168.0, 88.0],
                    },
                    WHITE,
                )
                .with_mask_blur(10.0),
            ],
        ),
        plate(
            "blur/gaussian-blur-one-dimension",
            vec![
                // A shape thin in one axis and long in the other, where a blur
                // that ran the same distance both ways would swallow it.
                Item::fill(
                    Shape::Rect {
                        min: [12.0, 60.0],
                        max: [116.0, 68.0],
                    },
                    WHITE,
                )
                .with_mask_blur(5.0),
            ],
        ),
        plate(
            "blur/solid-color-ovals-mask-blur-tiny-sigma",
            vec![Item::fill(
                Shape::Oval {
                    min: [16.0, 44.0],
                    max: [112.0, 84.0],
                },
                WHITE,
            )
            .with_mask_blur(0.4)],
        ),
    ];

    // Single-sampled, and not by preference. A backdrop filter has to read
    // what is already in the target, and a multisampled pass here must clear
    // rather than preserve -- preserving would need a resolved buffer copied
    // back into a multisampled one. So the two do not compose, and a scene
    // asking for both is refused by the backend rather than drawn wrongly.
    scenes.push(
        grouped(
            "blur/can-render-backdrop-blur",
            LayerSpec::default().with_backdrop_blur(6.0),
            Some([24.0, 44.0, 104.0, 84.0]),
        )
        .with_samples(1),
    );
    scenes.push(
        grouped(
            "blur/can-render-backdrop-blur-huge-sigma",
            LayerSpec::default().with_backdrop_blur(40.0),
            Some([24.0, 44.0, 104.0, 84.0]),
        )
        .with_samples(1),
    );
    scenes.push(grouped(
        "blur/can-render-bounded-blur",
        LayerSpec::default().with_blur(6.0),
        Some([16.0, 32.0, 112.0, 96.0]),
    ));
    scenes
}

/// `aiks_dl_unittests.cc` and `aiks_dl_opacity_unittests.cc`, for the scenes
/// about grouping rather than about any one shape.
fn layers() -> Vec<Scene> {
    vec![
        grouped("dl/can-save-layer-standalone", LayerSpec::default(), None),
        grouped(
            "dl/translucent-save-layer-draws-correctly",
            LayerSpec {
                alpha: 0.45,
                ..LayerSpec::default()
            },
            None,
        ),
        grouped(
            "dl/can-perform-save-layer-with-bounds",
            LayerSpec::default(),
            // Bounds narrower than the contents, which is the case a target
            // smaller than the frame exists for and the one where a wrong
            // origin shows as a shift rather than as a crop.
            Some([40.0, 40.0, 100.0, 90.0]),
        ),
        grouped(
            "dl/can-render-destructive-save-layer",
            LayerSpec {
                // Replaces rather than composites, so the group erases what is
                // under it including the ground.
                blend: BlendMode::Src,
                ..LayerSpec::default()
            },
            Some([24.0, 34.0, 108.0, 94.0]),
        ),
        Scene::tree(
            "dl/sibling-save-layer-bounds-are-respected",
            vec![
                Node::Layer {
                    layer: LayerSpec {
                        alpha: 0.6,
                        ..LayerSpec::default()
                    },
                    bounds: Some([8.0, 8.0, 64.0, 64.0]),
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [128.0, 128.0],
                        },
                        RED,
                    )))],
                },
                // A second group beside it: each is confined to its own
                // bounds, and one leaking into the other is what this catches.
                Node::Layer {
                    layer: LayerSpec {
                        alpha: 0.6,
                        ..LayerSpec::default()
                    },
                    bounds: Some([64.0, 64.0, 120.0, 120.0]),
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(
                        Item::fill(
                            Shape::Rect {
                                min: [0.0, 0.0],
                                max: [128.0, 128.0],
                            },
                            BLUE,
                        )
                        .with_blend(BlendMode::SrcOver),
                    ))],
                },
            ],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/can-render-tiny-overlapping-subpasses",
            (0..6)
                .map(|i| {
                    let x = 20.0 + i as f32 * 15.0;
                    Node::Layer {
                        layer: LayerSpec {
                            alpha: 0.7,
                            ..LayerSpec::default()
                        },
                        bounds: Some([x, 50.0, x + 22.0, 78.0]),
                        transform: Transform::default(),
                        children: vec![Node::Draw(Box::new(
                            Item::fill(
                                Shape::Circle {
                                    center: [x + 11.0, 64.0],
                                    radius: 12.0,
                                },
                                [0.3 + 0.12 * i as f32, 0.9 - 0.1 * i as f32, 1.0, 1.0],
                            )
                            .with_blend(BlendMode::SrcOver),
                        ))],
                    }
                })
                .collect(),
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

/// `aiks_dl_runtime_effect_unittests.cc`.
///
/// That file's scenes are a caller's shaders doing things no material does --
/// which is the whole category, so what can be shown here is bounded by the
/// one fixture program rather than by the renderer. What these plates
/// establish is that a caller's program reaches the picture at all, through a
/// shape, through a transform, and beside the built-in shader.
fn runtime_effect() -> Vec<Scene> {
    let effect = |threshold: f32| Fill::RuntimeEffect {
        uniforms: crate::fixture::effect_uniforms(RED, BLUE, threshold),
    };
    vec![
        plate(
            "effect/can-render-runtime-effect",
            vec![Item::filled(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                effect(0.0),
            )],
        ),
        plate(
            "effect/runtime-effect-can-precompile",
            vec![Item::filled(
                // A shape rather than the frame, so what is outside it shows
                // that the program filled geometry rather than everything.
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 46.0,
                },
                effect(-0.25),
            )],
        ),
        plate(
            "effect/runtime-effect-with-transform",
            vec![Item::filled(
                Shape::Rect {
                    min: [24.0, 40.0],
                    max: [104.0, 88.0],
                },
                effect(0.0),
            )
            // The shape turns; the split does not, because it is computed from
            // the fragment's place in clip space and a caller's program owes
            // nothing to a transform it was never given.
            .with_transform(Transform {
                rotate: 0.4,
                ..Transform::default()
            })],
        ),
    ]
}

/// A matrix that magnifies about a point and puts the result somewhere.
///
/// A filter's matrix acts on the finished image in device pixels, so a bare
/// scale magnifies about the frame's own corner and throws the result off the
/// plate. Composing the move in is what makes the picture, and it is the part
/// a caller reading `from_scale` would not think to do.
fn magnify(about: Vec2, factor: f32, to: Vec2) -> Affine2 {
    Affine2::from_translation(to)
        * Affine2::from_scale(Vec2::splat(factor))
        * Affine2::from_translation(-about)
}

/// The matrix image filter scenes, from the basic and base files.
///
/// What they show is the distinction between filtering a finished image and
/// drawing under a transform, which is the only reason both exist. A plate
/// cannot state that on its own -- both put the shape in the same place -- so
/// each of these is drawn beside the other, and the difference is in how the
/// edges and the interior arrived rather than in where they are.
fn image_filters() -> Vec<Scene> {
    let card = Shape::RoundedRect {
        min: [8.0, 8.0],
        max: [40.0, 40.0],
        radius: 6.0,
    };
    vec![
        plate(
            "basic/matrix-image-filter-magnify",
            vec![
                // The same card twice: once resampled to twice its size, once
                // drawn at twice its size, side by side so a reader comparing
                // the two edges compares them in one picture.
                //
                // The filter magnifies about the device origin, so putting the
                // result somewhere means composing the move into the matrix --
                // not into the item's transform, which would move the source
                // and then magnify the move as well.
                Item::filled(card.clone(), ramp()).with_image_filter(ImageFilter::Matrix {
                    transform: magnify(Vec2::new(24.0, 24.0), 2.0, Vec2::new(36.0, 64.0)),
                }),
                Item::filled(card.clone(), ramp())
                    .with_blend(BlendMode::SrcOver)
                    .with_transform(Transform {
                        scale: [2.0, 2.0],
                        translate: [44.0, 16.0],
                        ..Transform::default()
                    }),
            ],
        ),
        plate(
            "basic/massive-scaling-matrix-image-filter",
            vec![Item::fill(
                Shape::Rect {
                    min: [60.0, 60.0],
                    max: [68.0, 68.0],
                },
                YELLOW,
            )
            // Twelve times, which magnifies eight pixels to almost the plate.
            // A resample this large is where the layer's own resolution stops
            // being an implementation detail and becomes the picture.
            .with_image_filter(ImageFilter::Matrix {
                transform: magnify(Vec2::splat(64.0), 12.0, Vec2::splat(64.0)),
            })],
        ),
        Scene::tree(
            "dl/matrix-save-layer-filter",
            vec![Node::Layer {
                layer: LayerSpec {
                    // The group is resampled on the way back rather than its
                    // contents drawn larger, which is what a matrix filter on
                    // a save layer means.
                    matrix: Some(Transform {
                        scale: [2.0, 2.0],
                        translate: [-32.0, -32.0],
                        ..Transform::default()
                    }),
                    ..LayerSpec::default()
                },
                bounds: Some([32.0, 32.0, 96.0, 96.0]),
                transform: Transform::default(),
                children: vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Circle {
                            center: [56.0, 56.0],
                            radius: 18.0,
                        },
                        RED,
                    ))),
                    Node::Draw(Box::new(
                        Item::fill(
                            Shape::Circle {
                                center: [76.0, 76.0],
                                radius: 18.0,
                            },
                            BLUE,
                        )
                        .with_blend(BlendMode::SrcOver),
                    )),
                ],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}
