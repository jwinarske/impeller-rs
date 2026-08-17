//! Scenes as data, and the corpus of them.
//!
//! One corpus, many executions: the same scenes drive golden comparison,
//! cross-backend conformance, performance runs, and on-device runs. A new
//! feature adds scenes once and every execution mode picks them up, which is
//! what keeps the authoring cost flat as the matrix grows.

use crate::shape::Shape;
use glam::{Affine2, Vec2};
use impeller_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
use impeller_hal::{BlendMode, Extent2D};

/// An affine transform, as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub scale: [f32; 2],
    /// Rotation in radians, applied after scale and before translation.
    pub rotate: f32,
    pub translate: [f32; 2],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            scale: [1.0, 1.0],
            rotate: 0.0,
            translate: [0.0, 0.0],
        }
    }
}

impl Transform {
    pub fn translate(x: f32, y: f32) -> Self {
        Self {
            translate: [x, y],
            ..Default::default()
        }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Self {
            scale: [x, y],
            ..Default::default()
        }
    }

    pub fn to_affine(self) -> Affine2 {
        Affine2::from_scale_angle_translation(
            Vec2::from(self.scale),
            self.rotate,
            Vec2::from(self.translate),
        )
    }
}

/// A stroke's parameters, as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeSpec {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
}

impl StrokeSpec {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
        }
    }

    pub fn to_style(self) -> StrokeStyle {
        StrokeStyle {
            width: self.width,
            cap: self.cap,
            join: self.join,
            miter_limit: self.miter_limit,
        }
    }
}

/// A colour stop, as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// Linear colour with straight alpha.
    pub color: [f32; 4],
    pub offset: f32,
}

impl Stop {
    pub fn new(color: [f32; 4], offset: f32) -> Self {
        Self { color, offset }
    }
}

/// What fills a shape.
///
/// Kept as data alongside the geometry so a gradient scene serializes with
/// everything else, rather than needing code to reconstruct it.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid([f32; 4]),
    /// A gradient between two points in the item's own coordinate space, so it
    /// travels through the item's transform with the geometry.
    LinearGradient {
        start: [f32; 2],
        end: [f32; 2],
        stops: Vec<Stop>,
    },
}

/// One thing to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub shape: Shape,
    /// Stroke the shape rather than filling it.
    pub stroke: Option<StrokeSpec>,
    pub transform: Transform,
    pub fill: Fill,
    pub blend: BlendMode,
}

impl Item {
    pub fn fill(shape: Shape, color: [f32; 4]) -> Self {
        Self {
            shape,
            stroke: None,
            transform: Transform::default(),
            fill: Fill::Solid(color),
            blend: BlendMode::Src,
        }
    }

    /// A shape filled with a gradient between two points in its own space.
    pub fn gradient(shape: Shape, start: [f32; 2], end: [f32; 2], stops: Vec<Stop>) -> Self {
        Self {
            shape,
            stroke: None,
            transform: Transform::default(),
            fill: Fill::LinearGradient { start, end, stops },
            blend: BlendMode::Src,
        }
    }

    pub fn stroke(shape: Shape, spec: StrokeSpec, color: [f32; 4]) -> Self {
        Self {
            shape,
            stroke: Some(spec),
            transform: Transform::default(),
            fill: Fill::Solid(color),
            blend: BlendMode::Src,
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }
}

/// A named scene: everything needed to render one comparable image.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub name: &'static str,
    pub size: Extent2D,
    pub background: [f32; 4],
    /// MSAA sample count. 1 renders aliased.
    pub samples: u32,
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new(name: &'static str, items: Vec<Item>) -> Self {
        Self {
            name,
            size: Extent2D::new(128, 128),
            background: [0.0, 0.0, 0.0, 1.0],
            samples: 1,
            items,
        }
    }

    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples;
        self
    }

    pub fn with_background(mut self, background: [f32; 4]) -> Self {
        self.background = background;
        self
    }

    /// How closely two implementations must agree on this scene.
    ///
    /// The rule is where the value came from, not what the picture looks like:
    /// **exact where a value is transported, tolerant where it is computed per
    /// fragment.** A solid fill copies a colour through the pipeline, and any
    /// difference there is a defect. A gradient evaluates one, a blend converts
    /// an intermediate result to fixed point, and a multisample resolve
    /// averages — none of which the specification requires to be bit-identical
    /// across implementations, since shader arithmetic is permitted some error
    /// and compilers may fuse operations differently.
    ///
    /// Assigning this per scene by hand would drift as the corpus grows, and
    /// would let a genuine divergence be waved through by loosening one entry.
    pub fn tolerance(&self) -> crate::image::Tolerance {
        let computed = self.samples > 1
            || self.items.iter().any(|item| {
                item.blend == BlendMode::SrcOver || matches!(item.fill, Fill::LinearGradient { .. })
            });
        if computed {
            crate::image::Tolerance::ROUNDING
        } else {
            crate::image::Tolerance::EXACT
        }
    }
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
const GREEN: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
const BLUE: [f32; 4] = [0.2, 0.2, 1.0, 1.0];

/// The scene corpus.
///
/// Deliberately small and varied rather than large: each scene is here because
/// it exercises something the others do not, so a failure names a capability
/// rather than merely a picture. Regression pins are appended as bugs are
/// fixed, and that set only grows.
pub fn corpus() -> Vec<Scene> {
    vec![
        Scene::new(
            "rect-fill",
            vec![Item::fill(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                WHITE,
            )],
        ),
        Scene::new(
            "circle-fill",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 48.0,
                },
                WHITE,
            )],
        ),
        // Concave, so it must not take the convex fan path.
        Scene::new(
            "concave-polygon",
            vec![Item::fill(
                Shape::Polygon(vec![
                    [16.0, 16.0],
                    [112.0, 16.0],
                    [112.0, 64.0],
                    [64.0, 64.0],
                    [64.0, 112.0],
                    [16.0, 112.0],
                ]),
                WHITE,
            )],
        ),
        Scene::new(
            "overlapping-opaque",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [16.0, 16.0],
                        max: [80.0, 80.0],
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [48.0, 48.0],
                        max: [112.0, 112.0],
                    },
                    BLUE,
                ),
            ],
        ),
        Scene::new(
            "translucent-stack",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [8.0, 8.0],
                        max: [120.0, 120.0],
                    },
                    BLUE,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [56.0, 56.0],
                        radius: 40.0,
                    },
                    [1.0, 0.0, 0.0, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Circle {
                        center: [80.0, 80.0],
                        radius: 40.0,
                    },
                    [0.0, 1.0, 0.0, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        Scene::new(
            "stroke-caps-and-joins",
            vec![
                Item::stroke(
                    Shape::Polygon(vec![[24.0, 32.0], [64.0, 96.0], [104.0, 32.0]]),
                    StrokeSpec {
                        width: 10.0,
                        cap: LineCap::Round,
                        join: LineJoin::Round,
                        miter_limit: 4.0,
                    },
                    GREEN,
                ),
                Item::stroke(
                    Shape::Cubic {
                        start: [16.0, 112.0],
                        c0: [48.0, 64.0],
                        c1: [80.0, 160.0],
                        end: [112.0, 112.0],
                    },
                    StrokeSpec::new(6.0),
                    WHITE,
                ),
            ],
        ),
        Scene::new(
            "transformed",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [32.0, 32.0],
                    },
                    RED,
                )
                .with_transform(Transform::translate(16.0, 16.0)),
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [16.0, 16.0],
                    },
                    GREEN,
                )
                .with_transform(Transform {
                    scale: [3.0, 1.5],
                    rotate: 0.4,
                    translate: [64.0, 64.0],
                }),
            ],
        ),
        // The same content as circle-fill, multisampled: the pair is what makes
        // an antialiasing regression visible as a diff rather than a judgement.
        Scene::new(
            "circle-antialiased",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 48.0,
                },
                WHITE,
            )],
        )
        .with_samples(4),
        // Gradients are where the two backends most easily diverge: one sends
        // the paint as push constants, the other as individually-set uniforms,
        // and the fragment locates itself from an interpolated clip position
        // whose orientation the two APIs disagree about. Comparing them is the
        // point of having these in the corpus rather than only in a suite
        // someone remembers to run twice.
        Scene::new(
            "gradient-horizontal",
            vec![Item::gradient(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                [8.0, 0.0],
                [120.0, 0.0],
                vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
            )],
        ),
        // Vertical as well as horizontal: an axis mix-up leaves one of the two
        // looking perfectly correct.
        Scene::new(
            "gradient-vertical",
            vec![Item::gradient(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 52.0,
                },
                [0.0, 12.0],
                [0.0, 116.0],
                vec![
                    Stop::new(RED, 0.0),
                    Stop::new(GREEN, 0.5),
                    Stop::new(BLUE, 1.0),
                ],
            )],
        ),
        // Under a transform, so the endpoints are exercised through the same
        // mapping the geometry takes rather than only through the identity.
        Scene::new(
            "gradient-transformed",
            vec![Item::gradient(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [64.0, 64.0],
                },
                [0.0, 0.0],
                [64.0, 0.0],
                vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
            )
            .with_transform(Transform {
                scale: [1.5, 1.5],
                rotate: 0.6,
                translate: [40.0, 16.0],
            })],
        ),
        Scene::new(
            "curve-antialiased",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [8.0, 96.0],
                    c0: [48.0, 8.0],
                    c1: [80.0, 152.0],
                    end: [120.0, 40.0],
                },
                StrokeSpec {
                    width: 8.0,
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                },
                WHITE,
            )],
        )
        .with_samples(4),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_distinct_name() {
        // Names key report rows and tolerance tables, so a duplicate would make
        // two scenes indistinguishable in results.
        let mut names: Vec<&str> = corpus().iter().map(|s| s.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate scene name");
    }

    #[test]
    fn every_scene_draws_something() {
        for scene in corpus() {
            assert!(!scene.items.is_empty(), "{} draws nothing", scene.name);
            assert!(!scene.size.is_empty(), "{} has no area", scene.name);
            assert!(scene.samples.is_power_of_two(), "{}", scene.name);
        }
    }

    #[test]
    fn the_corpus_covers_more_than_one_kind_of_work() {
        let scenes = corpus();
        assert!(scenes.iter().any(|s| s.samples > 1), "no antialiased scene");
        assert!(
            scenes
                .iter()
                .any(|s| s.items.iter().any(|i| i.stroke.is_some())),
            "no stroked scene"
        );
        assert!(
            scenes
                .iter()
                .any(|s| s.items.iter().any(|i| i.blend == BlendMode::SrcOver)),
            "no blended scene"
        );
        assert!(
            scenes
                .iter()
                .any(|s| s.items.iter().any(|i| i.transform != Transform::default())),
            "no transformed scene"
        );
    }

    #[test]
    fn a_transform_composes_scale_rotation_and_translation() {
        let t = Transform {
            scale: [2.0, 2.0],
            rotate: 0.0,
            translate: [10.0, 5.0],
        };
        let p = t.to_affine().transform_point2(Vec2::new(1.0, 1.0));
        assert!((p - Vec2::new(12.0, 7.0)).length() < 1e-5);
    }
}
