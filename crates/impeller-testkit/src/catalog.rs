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

use crate::scene::{Fill, Item, LayerSpec, Node, Scene, Stop, StrokeSpec, Transform};
use crate::shape::Shape;
use impeller_geometry::stroke::{LineCap, LineJoin};
use impeller_geometry::FillRule;
use impeller_hal::{BlendMode, TileMode};

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
