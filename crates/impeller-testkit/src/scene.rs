//! Scenes as data, and the corpus of them.
//!
//! One corpus, many executions: the same scenes drive golden comparison,
//! cross-backend conformance, performance runs, and on-device runs. A new
//! feature adds scenes once and every execution mode picks them up, which is
//! what keeps the authoring cost flat as the matrix grows.

use crate::shape::Shape;
use glam::{Affine2, Vec2};
use impeller_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
use impeller_geometry::FillRule;
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
    /// A gradient outward from a centre, reaching its last stop at `radius`.
    RadialGradient {
        center: [f32; 2],
        radius: f32,
        stops: Vec<Stop>,
    },
    /// A gradient around a centre, between two angles in radians.
    SweepGradient {
        center: [f32; 2],
        start_angle: f32,
        end_angle: f32,
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
    /// Confine this item to a rectangle, given in the item's own space as
    /// `[left, top, right, bottom]` and carried through its transform.
    pub clip: Option<[f32; 4]>,
    /// Confine this item to an arbitrary shape, in the item's own space.
    ///
    /// Needs the stencil rather than the scissor, and so exercises a quite
    /// different path from [`Self::clip`] even though both narrow what the item
    /// may reach. Applied per item: the clip is built before the item and
    /// stepped back after it, so items stay independent of each other.
    pub clip_shape: Option<Shape>,
}

impl Item {
    pub fn fill(shape: Shape, color: [f32; 4]) -> Self {
        Self {
            shape,
            stroke: None,
            transform: Transform::default(),
            fill: Fill::Solid(color),
            blend: BlendMode::Src,
            clip: None,
            clip_shape: None,
        }
    }

    /// A shape filled with a gradient between two points in its own space.
    pub fn gradient(shape: Shape, start: [f32; 2], end: [f32; 2], stops: Vec<Stop>) -> Self {
        Self::filled(shape, Fill::LinearGradient { start, end, stops })
    }

    /// A shape filled with any fill.
    pub fn filled(shape: Shape, fill: Fill) -> Self {
        Self {
            shape,
            stroke: None,
            transform: Transform::default(),
            fill,
            blend: BlendMode::Src,
            clip: None,
            clip_shape: None,
        }
    }

    pub fn stroke(shape: Shape, spec: StrokeSpec, color: [f32; 4]) -> Self {
        Self {
            shape,
            stroke: Some(spec),
            transform: Transform::default(),
            fill: Fill::Solid(color),
            blend: BlendMode::Src,
            clip: None,
            clip_shape: None,
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// Confine this item to `[left, top, right, bottom]` in its own space.
    pub fn with_clip(mut self, clip: [f32; 4]) -> Self {
        self.clip = Some(clip);
        self
    }

    /// Confine this item to a shape in its own space, through the stencil.
    pub fn with_clip_shape(mut self, shape: Shape) -> Self {
        self.clip_shape = Some(shape);
        self
    }

    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Whether the public API will draw this item from a distance field rather
    /// than from triangles.
    ///
    /// Mirrors the condition `Canvas::draw_rrect` applies. Stated here because
    /// the tolerance is derived from what a scene does, and what this one does
    /// depends on which path the call takes -- so a scene that says "rounded
    /// rectangle, filled, antialiased" is saying "coverage from a distance
    /// field", whether or not it knows the name for it.
    fn is_analytic(&self) -> bool {
        matches!(self.shape, crate::shape::Shape::RoundedRect { radius, .. } if radius > 0.0)
            && self.stroke.is_none()
            && matches!(self.fill, Fill::Solid(_))
    }
}

/// How a group is composited back onto what is underneath it.
///
/// The two parts of a layer that mean anything: there is no shape to fill and
/// no geometry to stroke, so a paint would mostly be fields that do nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSpec {
    /// Standard deviation of a blur over the finished group, in device pixels.
    /// Zero for none.
    pub blur: f32,
    pub alpha: f32,
    pub blend: BlendMode,
}

impl Default for LayerSpec {
    fn default() -> Self {
        Self {
            blur: 0.0,
            alpha: 1.0,
            blend: BlendMode::SrcOver,
        }
    }
}

impl LayerSpec {
    pub fn opacity(alpha: f32) -> Self {
        Self {
            alpha,
            ..Self::default()
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_blur(mut self, blur: f32) -> Self {
        self.blur = blur;
        self
    }
}

/// One entry in a scene: something to draw, or a group to draw and composite.
///
/// A scene is a tree rather than a list because a layer contains things. That
/// is the only reason — everything else about a scene stayed flat, and the flat
/// constructors below are unchanged, so a scene that has no layers reads
/// exactly as it did.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Draw(Item),
    /// A group rendered into a target of its own and composited back.
    ///
    /// `bounds` is the region the group promises to stay inside, in the space
    /// the layer opens in, and is what lets the target be smaller than the
    /// frame. `None` asks for a full-size target, which is what a caller who
    /// does not know gets.
    Layer {
        layer: LayerSpec,
        bounds: Option<[f32; 4]>,
        /// Applied before the layer opens, so it moves the bounds along with
        /// the contents rather than only the contents.
        transform: Transform,
        children: Vec<Node>,
    },
}

impl From<Item> for Node {
    fn from(item: Item) -> Self {
        Self::Draw(item)
    }
}

impl Node {
    /// A group with a full-size target, which is what a caller states when it
    /// does not know what the group covers.
    pub fn layer(layer: LayerSpec, children: Vec<Node>) -> Self {
        Self::Layer {
            layer,
            bounds: None,
            transform: Transform::default(),
            children,
        }
    }

    /// A group that promises to stay inside `[left, top, right, bottom]`.
    pub fn bounded_layer(layer: LayerSpec, bounds: [f32; 4], children: Vec<Node>) -> Self {
        Self::Layer {
            layer,
            bounds: Some(bounds),
            transform: Transform::default(),
            children,
        }
    }

    pub fn with_transform(mut self, applied: Transform) -> Self {
        if let Self::Layer { transform, .. } = &mut self {
            *transform = applied;
        }
        self
    }

    /// Every item in this subtree, for the derivations that ask what a scene
    /// contains without caring how it is grouped.
    fn items(&self) -> Box<dyn Iterator<Item = &Item> + '_> {
        match self {
            Self::Draw(item) => Box::new(std::iter::once(item)),
            Self::Layer { children, .. } => Box::new(children.iter().flat_map(Node::items)),
        }
    }

    fn items_mut(&mut self) -> Box<dyn Iterator<Item = &mut Item> + '_> {
        match self {
            Self::Draw(item) => Box::new(std::iter::once(item)),
            Self::Layer { children, .. } => Box::new(children.iter_mut().flat_map(Node::items_mut)),
        }
    }

    /// Whether this subtree composites a group at all.
    fn has_layer(&self) -> bool {
        match self {
            Self::Draw(_) => false,
            Self::Layer { .. } => true,
        }
    }

    fn has_bounded_layer(&self) -> bool {
        match self {
            Self::Draw(_) => false,
            Self::Layer {
                bounds, children, ..
            } => bounds.is_some() || children.iter().any(Node::has_bounded_layer),
        }
    }

    fn unbound(&mut self) {
        if let Self::Layer {
            bounds, children, ..
        } = self
        {
            *bounds = None;
            children.iter_mut().for_each(Node::unbound);
        }
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
    pub items: Vec<Node>,
}

impl Scene {
    /// A scene that draws a flat list of items.
    ///
    /// Kept alongside [`Self::tree`] rather than replaced by it because most
    /// scenes have no groups, and making every one of them say so would be
    /// noise in the place a reader looks to see what a scene draws.
    pub fn new(name: &'static str, items: Vec<Item>) -> Self {
        Self::tree(name, items.into_iter().map(Node::Draw).collect())
    }

    /// A scene whose entries may be groups.
    pub fn tree(name: &'static str, items: Vec<Node>) -> Self {
        Self {
            name,
            size: Extent2D::new(128, 128),
            background: [0.0, 0.0, 0.0, 1.0],
            samples: 1,
            items,
        }
    }

    /// Every item the scene draws, whatever it is grouped inside.
    ///
    /// Grouping is what a layer is for, and every derivation below asks what a
    /// scene contains rather than how it is arranged, so they all walk the tree
    /// through this rather than each learning its shape.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().flat_map(Node::items)
    }

    /// The same, for a caller that alters a scene to check a test can fail.
    pub fn items_mut(&mut self) -> impl Iterator<Item = &mut Item> {
        self.items.iter_mut().flat_map(Node::items_mut)
    }

    /// Whether any group in this scene was told the region it covers.
    pub fn has_bounded_layer(&self) -> bool {
        self.items.iter().any(Node::has_bounded_layer)
    }

    /// The same scene with every layer asking for a full-size target.
    ///
    /// Bounds are an optimization: the same drawing, into a target that happens
    /// to be smaller. So this is the scene that must render identically, and
    /// producing it by stripping the original rather than by writing it out
    /// twice is what keeps the two from drifting apart.
    pub fn unbounded(&self) -> Self {
        let mut stripped = self.clone();
        stripped.items.iter_mut().for_each(Node::unbound);
        stripped
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
        // Any fill that is not a plain colour is evaluated per fragment, so
        // this asks what the fill is not rather than listing the kinds that
        // are. Enumerating them meant a new gradient kind silently inherited
        // the exact rule and failed the moment it was added.
        // A layer is composited back with a blend and an alpha, which is the
        // same per-fragment arithmetic a translucent draw does, so a scene that
        // groups anything is computed whatever its items are.
        // A distance field first, because what it permits is smaller than the
        // multisample budget and needs no count: the derivative that sets the
        // edge width is implementation-defined, so coverage differs by a unit
        // or two along the whole edge rather than by a sample's worth at a few
        // pixels.
        //
        // Conditional on multisampling too, since that is what makes the
        // executor ask for antialiasing and the call take that path. Without
        // it this would loosen the aliased rounded-rectangle scene, which is
        // drawn from triangles and should still compare exactly.
        if self.samples > 1 && self.items().any(Item::is_analytic) {
            return crate::image::Tolerance::ANALYTIC;
        }
        // Multisampling first, because it permits something the others do not:
        // a whole sample's worth of difference at an edge, on a few pixels. The
        // rest permit a unit everywhere and nothing more.
        if self.samples > 1 {
            return crate::image::Tolerance::MULTISAMPLED;
        }
        let computed = self.items.iter().any(Node::has_layer)
            || self.items().any(|item| {
                item.blend == BlendMode::SrcOver || !matches!(item.fill, Fill::Solid(_))
            });
        if computed {
            crate::image::Tolerance::ROUNDING
        } else {
            crate::image::Tolerance::EXACT
        }
    }

    /// Whether a device can render this scene at all.
    ///
    /// Derived from what the scene contains, for the same reason the tolerance
    /// is: a requirement written alongside the scene is one that can be
    /// forgotten, and a scene needing a capability nobody declared would be
    /// reported as a backend regression rather than as the known gap it is.
    ///
    /// The distinction this draws matters to the cross-backend comparison. A
    /// scene refused by a device that the scene says needs nothing special is a
    /// defect; a scene refused by a device the scene says cannot render it is a
    /// gap, and the corpus reports the second as coverage it did not get rather
    /// than as a pass.
    pub fn supported_by(&self, capabilities: &impeller_hal::Capabilities) -> bool {
        if !capabilities.sample_counts.supports(self.samples) {
            return false;
        }
        if !capabilities.advanced_blend && self.items().any(|item| item.blend.is_advanced()) {
            return false;
        }
        true
    }
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
const GREEN: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
const BLUE: [f32; 4] = [0.2, 0.2, 1.0, 1.0];

/// A backdrop and three overlapping translucent circles, one per mode.
///
/// The backdrop is a gradient rather than a flat color because several of these
/// modes are functions of the backdrop's value — dodge, burn and the two
/// contrast modes all behave differently at each end — and a flat backdrop
/// would exercise one point of each curve. The circles overlap each other so
/// the second and third blend against a result the first produced, which is
/// what a coherent blend has to get right and an incoherent one does not.
fn advanced_blend_items(modes: &[BlendMode; 3]) -> Vec<Item> {
    let mut items = vec![Item::filled(
        Shape::Rect {
            min: [0.0, 0.0],
            max: [128.0, 128.0],
        },
        Fill::LinearGradient {
            start: [0.0, 0.0],
            end: [128.0, 128.0],
            stops: vec![
                Stop::new([0.05, 0.1, 0.35, 1.0], 0.0),
                Stop::new([0.6, 0.55, 0.2, 1.0], 0.5),
                Stop::new([0.95, 0.9, 0.85, 1.0], 1.0),
            ],
        },
    )];
    let placements = [
        ([48.0, 44.0], [0.9, 0.35, 0.2, 0.8]),
        ([80.0, 56.0], [0.25, 0.7, 0.85, 0.8]),
        ([64.0, 88.0], [0.6, 0.85, 0.3, 0.8]),
    ];
    for (mode, (center, color)) in modes.iter().zip(placements) {
        items.push(
            Item::fill(
                Shape::Circle {
                    center,
                    radius: 34.0,
                },
                color,
            )
            .with_blend(*mode),
        );
    }
    items
}

/// Three sharp elbows side by side, for a scene that varies the join.
///
/// Open, so the ends carry caps and the corner carries a join, and sharp,
/// because the three joins differ by how they fill the outside of a corner and
/// a shallow one leaves almost nothing to differ over.
fn elbows() -> Vec<Vec<[f32; 2]>> {
    (0..3)
        .map(|i| {
            let x = 22.0 + i as f32 * 42.0;
            vec![[x - 14.0, 96.0], [x, 40.0], [x + 14.0, 96.0]]
        })
        .collect()
}

/// A five-pointed star as one closed path, which crosses itself five times.
fn pentagram() -> Vec<[f32; 2]> {
    (0..5)
        .map(|k| {
            let angle = (-90.0 + k as f32 * 144.0).to_radians();
            [64.0 + 52.0 * angle.cos(), 64.0 + 52.0 * angle.sin()]
        })
        .collect()
}

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
        // The separable blend modes, three per scene so the mixing is visible
        // where they overlap each other as well as the backdrop. These need an
        // advanced-blend extension, so a device without one reports them as a
        // declared gap through `supported_by` rather than failing to render.
        Scene::new(
            "advanced-blend-darkening",
            advanced_blend_items(&[BlendMode::Multiply, BlendMode::ColorBurn, BlendMode::Darken]),
        ),
        Scene::new(
            "advanced-blend-lightening",
            advanced_blend_items(&[BlendMode::Screen, BlendMode::ColorDodge, BlendMode::Lighten]),
        ),
        Scene::new(
            "advanced-blend-contrast",
            advanced_blend_items(&[
                BlendMode::Overlay,
                BlendMode::HardLight,
                BlendMode::SoftLight,
            ]),
        ),
        Scene::new(
            "advanced-blend-non-separable",
            advanced_blend_items(&[BlendMode::Hue, BlendMode::Saturation, BlendMode::Color]),
        ),
        Scene::new(
            "advanced-blend-luminosity",
            advanced_blend_items(&[BlendMode::Luminosity, BlendMode::Hue, BlendMode::Luminosity]),
        ),
        Scene::new(
            "advanced-blend-inverting",
            advanced_blend_items(&[
                BlendMode::Difference,
                BlendMode::Exclusion,
                BlendMode::Multiply,
            ]),
        ),
        // Clipping. A scissor is exact, so these compare bit-for-bit between
        // backends and devices -- which is what makes them worth having:
        // an off-by-one or a mirrored axis shows up as a hard failure rather
        // than as something within tolerance.
        Scene::new(
            "clipped-circle",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 56.0,
                },
                RED,
            )
            // Deliberately off-center, and clipping the circle on three
            // sides but not the fourth, so a mirrored or transposed clip
            // produces a different picture rather than the same one.
            .with_clip([20.0, 8.0, 100.0, 72.0])],
        ),
        Scene::new(
            "clip-varies-between-draws",
            vec![
                // Overlapping bands, each clipped differently, with an
                // unclipped shape between them. Clip state persists until it is
                // set again, so this catches a clip leaking into a later draw
                // as well as one never being applied.
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    BLUE,
                )
                .with_clip([0.0, 0.0, 40.0, 128.0]),
                Item::fill(
                    Shape::Rect {
                        min: [48.0, 48.0],
                        max: [80.0, 80.0],
                    },
                    WHITE,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    GREEN,
                )
                .with_clip([96.0, 24.0, 128.0, 104.0]),
            ],
        ),
        Scene::new(
            "clip-follows-its-transform",
            vec![
                // The clip is stated in the item's own space, so it travels
                // through the rotation and scale with the shape. A clip applied
                // in device pixels instead would sit at the target's origin.
                Item::fill(
                    Shape::Rect {
                        min: [-40.0, -40.0],
                        max: [40.0, 40.0],
                    },
                    RED,
                )
                .with_transform(Transform {
                    scale: [1.0, 1.0],
                    rotate: std::f32::consts::FRAC_PI_2,
                    translate: [72.0, 56.0],
                })
                .with_clip([-40.0, -40.0, 10.0, 24.0]),
            ],
        ),
        // Clipping by a shape a rectangle cannot express, which goes through
        // the stencil rather than the scissor. Like the scissor scenes these
        // compare exactly: a pixel is either admitted or it is not, with no
        // per-fragment arithmetic to permit a difference.
        Scene::new(
            "shape-clipped-fill",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                RED,
            )
            // A triangle whose bounding box reaches three corners the triangle
            // itself misses, so clipping to its bounds would be a visibly
            // different picture.
            .with_clip_shape(Shape::Polygon(vec![
                [64.0, 12.0],
                [116.0, 104.0],
                [20.0, 92.0],
            ]))],
        ),
        Scene::new(
            "shape-clip-and-scissor-together",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 60.0,
                },
                GREEN,
            )
            // Both mechanisms at once on one item, which is the arrangement
            // that would break if either were applied in place of the other
            // rather than alongside it.
            .with_clip_shape(Shape::Polygon(vec![
                [10.0, 118.0],
                [64.0, 8.0],
                [118.0, 118.0],
            ]))
            .with_clip([0.0, 0.0, 78.0, 128.0])],
        ),
        Scene::new(
            "shape-clip-follows-its-transform",
            vec![Item::fill(
                Shape::Rect {
                    min: [-56.0, -56.0],
                    max: [56.0, 56.0],
                },
                BLUE,
            )
            .with_transform(Transform {
                scale: [1.0, 1.0],
                // An eighth turn, which no scissor expresses: the clip becomes
                // a diamond and its bounding box is visibly larger.
                rotate: std::f32::consts::FRAC_PI_4,
                translate: [64.0, 64.0],
            })
            .with_clip_shape(Shape::Rect {
                min: [-38.0, -38.0],
                max: [38.0, 38.0],
            })],
        ),
        Scene::new(
            "stroke-polygon-and-curve",
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
            "gradient-radial",
            vec![Item::filled(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [124.0, 124.0],
                },
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                },
            )],
        ),
        Scene::new(
            "gradient-sweep",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 56.0,
                },
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::TAU,
                    stops: vec![
                        Stop::new(RED, 0.0),
                        Stop::new(GREEN, 0.5),
                        Stop::new(BLUE, 1.0),
                    ],
                },
            )],
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
        // A path that crosses itself, filled by each rule. The two differ only
        // in the middle: a pentagram wound once has a center the crossings
        // enclose twice, so non-zero fills it and even-odd leaves it hollow.
        //
        // One subpath rather than the two a corpus would otherwise reach for,
        // because that is the case that was wrong. A path of several subpaths
        // goes through the sweep, which has always honored the rule; a single
        // subpath is offered to a convexity test first, and a self-crossing
        // one whose turns all agree was called convex and fan-filled -- by a
        // routine with no notion of a fill rule, which therefore discarded it.
        Scene::new(
            "self-crossing-nonzero",
            vec![Item::filled(
                Shape::RuledPolygon {
                    points: pentagram(),
                    rule: FillRule::NonZero,
                },
                Fill::Solid(WHITE),
            )],
        ),
        Scene::new(
            "self-crossing-evenodd",
            vec![Item::filled(
                Shape::RuledPolygon {
                    points: pentagram(),
                    rule: FillRule::EvenOdd,
                },
                Fill::Solid(WHITE),
            )],
        ),
        // The stroke joins, one elbow each. A join is what fills the outside
        // of a corner, and the three fill it differently: a miter runs out to
        // the point where the two edges would meet, a bevel cuts straight
        // across, and a round arcs between. Nothing else in the corpus varies
        // this -- the scene that used to be named for it states one value.
        Scene::new(
            "stroke-joins",
            vec![
                (LineJoin::Miter, RED),
                (LineJoin::Round, GREEN),
                (LineJoin::Bevel, BLUE),
            ]
            .into_iter()
            .zip(elbows())
            .map(|((join, color), points)| {
                Item::stroke(
                    Shape::Polyline(points),
                    StrokeSpec {
                        width: 14.0,
                        // Butt, so the ends contribute nothing and the only
                        // difference between these is the corner.
                        cap: LineCap::Butt,
                        join,
                        miter_limit: 8.0,
                    },
                    color,
                )
            })
            .collect(),
        ),
        // The stroke caps, one segment each. A cap is what closes an open
        // end, so a corpus of closed shapes cannot exercise one however many
        // strokes it has -- which is what the corpus was.
        Scene::new(
            "stroke-caps",
            vec![
                (LineCap::Butt, RED),
                (LineCap::Square, GREEN),
                (LineCap::Round, BLUE),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (cap, color))| {
                let y = 32.0 + i as f32 * 32.0;
                Item::stroke(
                    Shape::Polyline(vec![[32.0, y], [96.0, y]]),
                    StrokeSpec {
                        width: 16.0,
                        cap,
                        join: LineJoin::Miter,
                        miter_limit: 4.0,
                    },
                    color,
                )
            })
            .collect(),
        ),
        // Rounded rectangles, which an interface is mostly made of and which
        // nothing else here draws. Two radii and a stroke: a modest one where
        // the straight edges still dominate, one large enough to be clamped to
        // half the shorter side and come out a stadium, and a stroked outline
        // where the corner arcs meet the straight runs and a tangent that was
        // slightly wrong shows as a kink.
        Scene::new(
            "rounded-rect",
            vec![
                Item::fill(
                    Shape::RoundedRect {
                        min: [12.0, 16.0],
                        max: [116.0, 60.0],
                        radius: 12.0,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::RoundedRect {
                        min: [12.0, 72.0],
                        max: [116.0, 116.0],
                        // Far past half the height, so the clamp is what
                        // decides the shape.
                        radius: 400.0,
                    },
                    BLUE,
                ),
            ],
        ),
        // Multisampled, so the executor asks for antialiasing and the public
        // call takes its analytic path -- which the corpus would otherwise
        // never reach, since every other scene here hands over a path.
        Scene::new(
            "rounded-rect-analytic",
            vec![Item::fill(
                Shape::RoundedRect {
                    min: [24.0, 24.0],
                    max: [104.0, 88.0],
                    radius: 22.0,
                },
                GREEN,
            )],
        )
        .with_samples(4),
        Scene::new(
            "rounded-rect-stroked",
            vec![Item::stroke(
                Shape::RoundedRect {
                    min: [20.0, 20.0],
                    max: [108.0, 108.0],
                    radius: 28.0,
                },
                StrokeSpec {
                    width: 9.0,
                    cap: LineCap::Butt,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                },
                GREEN,
            )],
        )
        .with_samples(4),
        // A blurred group, which is three passes rather than one: the contents,
        // then one per axis of a separable Gaussian. Two backends that agreed
        // on everything else could still differ here, since this is the only
        // thing that samples a target it just rendered, twice, with computed
        // weights.
        Scene::tree(
            "layer-blurred",
            vec![Node::layer(
                LayerSpec::default().with_blur(6.0),
                vec![
                    Item::fill(
                        Shape::Rect {
                            min: [32.0, 32.0],
                            max: [96.0, 72.0],
                        },
                        WHITE,
                    )
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 92.0],
                            radius: 18.0,
                        },
                        RED,
                    )
                    .into(),
                ],
            )],
        ),
        // Layers. Everything below here needs the scene to be a tree, and none
        // of it could be said at all while a scene was a flat list of items --
        // which is why layer compositing went uncompared across backends for as
        // long as it did.
        //
        // Group opacity is the reason layers exist. Two translucent circles
        // drawn directly show where they cross; the same pair made first and
        // faded once does not. A backend that composited per shape rather than
        // per group would differ exactly on the overlap.
        Scene::tree(
            "layer-group-opacity",
            vec![Node::layer(
                LayerSpec::opacity(0.55),
                vec![
                    Item::fill(
                        Shape::Circle {
                            center: [52.0, 64.0],
                            radius: 32.0,
                        },
                        RED,
                    )
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [80.0, 64.0],
                            radius: 32.0,
                        },
                        GREEN,
                    )
                    .into(),
                ],
            )],
        ),
        // A layer that meets what is underneath through a blend rather than
        // through the default. The composite is one draw of the whole group, so
        // this is the mode applied once to a finished image -- a different
        // thing from the same mode on each shape, and the pair above is what
        // makes the difference visible.
        Scene::tree(
            "layer-blended-composite",
            vec![
                Item::filled(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    Fill::LinearGradient {
                        start: [0.0, 0.0],
                        end: [128.0, 128.0],
                        stops: vec![
                            Stop::new([0.05, 0.1, 0.35, 1.0], 0.0),
                            Stop::new([0.9, 0.85, 0.4, 1.0], 1.0),
                        ],
                    },
                )
                .into(),
                Node::layer(
                    LayerSpec::opacity(0.8).with_blend(BlendMode::SrcOver),
                    vec![Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 36.0,
                        },
                        [0.2, 0.9, 0.7, 1.0],
                    )
                    .into()],
                ),
            ],
        ),
        // A layer given the region it covers, so its target is smaller than the
        // frame and sits at an offset inside it. Everything that maps between
        // spaces has to agree about where that target is: the gradient states
        // its endpoints in the scene's space and the geometry is tessellated in
        // device pixels, and the two are projected separately.
        Scene::tree(
            "layer-bounded",
            vec![Node::bounded_layer(
                LayerSpec::opacity(0.7),
                [24.0, 40.0, 96.0, 104.0],
                vec![
                    Item::filled(
                        Shape::Rect {
                            min: [24.0, 40.0],
                            max: [96.0, 104.0],
                        },
                        Fill::LinearGradient {
                            start: [24.0, 40.0],
                            end: [96.0, 104.0],
                            stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                        },
                    )
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [60.0, 72.0],
                            radius: 26.0,
                        },
                        [1.0, 1.0, 1.0, 0.7],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            )],
        ),
        // Layers nested, the inner one bounded and opened under a transform and
        // inside both kinds of clip. An inner target is placed within its
        // parent's rather than within the frame, so compositing it as though
        // the parent filled the frame puts it off by the parent's own offset --
        // which a single layer cannot catch and this does.
        Scene::tree(
            "layer-nested-clipped",
            vec![Node::bounded_layer(
                LayerSpec::opacity(0.75),
                [24.0, 40.0, 96.0, 104.0],
                vec![
                    Item::filled(
                        Shape::Rect {
                            min: [24.0, 40.0],
                            max: [96.0, 104.0],
                        },
                        Fill::LinearGradient {
                            start: [24.0, 40.0],
                            end: [96.0, 104.0],
                            stops: vec![
                                Stop::new([1.0, 0.25, 0.0, 1.0], 0.0),
                                Stop::new(BLUE, 1.0),
                            ],
                        },
                    )
                    .with_clip([30.0, 46.0, 90.0, 98.0])
                    .with_clip_shape(Shape::Polygon(vec![
                        [60.0, 44.0],
                        [92.0, 100.0],
                        [28.0, 100.0],
                    ]))
                    .into(),
                    Node::bounded_layer(
                        LayerSpec::opacity(0.5),
                        [38.0, 52.0, 82.0, 96.0],
                        vec![Item::fill(
                            Shape::Circle {
                                center: [60.0, 74.0],
                                radius: 22.0,
                            },
                            GREEN,
                        )
                        .into()],
                    )
                    .with_transform(Transform {
                        scale: [1.0, 1.0],
                        rotate: 0.0,
                        translate: [4.0, 6.0],
                    }),
                ],
            )],
        ),
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
            scenes.iter().any(|s| s.items().any(|i| i.stroke.is_some())),
            "no stroked scene"
        );
        assert!(
            scenes
                .iter()
                .any(|s| s.items().any(|i| i.blend == BlendMode::SrcOver)),
            "no blended scene"
        );
        assert!(
            scenes
                .iter()
                .any(|s| s.items().any(|i| i.transform != Transform::default())),
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

#[cfg(test)]
mod tolerance_tests {
    use super::*;
    use crate::image::Tolerance;

    fn rounded(radius: f32) -> Shape {
        Shape::RoundedRect {
            min: [10.0, 10.0],
            max: [90.0, 70.0],
            radius,
        }
    }

    #[test]
    fn only_a_scene_that_will_be_drawn_analytically_gets_that_budget() {
        // The budget is for coverage computed from a screen-space derivative,
        // and a scene drawn from triangles must not receive it just for
        // containing the same shape. Aliased, the call tessellates.
        let aliased = Scene::new("aliased", vec![Item::fill(rounded(12.0), WHITE)]);
        assert_eq!(aliased.tolerance(), Tolerance::EXACT);

        let antialiased =
            Scene::new("antialiased", vec![Item::fill(rounded(12.0), WHITE)]).with_samples(4);
        assert_eq!(antialiased.tolerance(), Tolerance::ANALYTIC);

        // A stroke is a different shape and is tessellated either way.
        let stroked = Scene::new(
            "stroked",
            vec![Item::stroke(rounded(12.0), StrokeSpec::new(4.0), WHITE)],
        )
        .with_samples(4);
        assert_eq!(stroked.tolerance(), Tolerance::MULTISAMPLED);

        // A radius of zero is a plain rectangle, with no distance field.
        let square = Scene::new("square", vec![Item::fill(rounded(0.0), WHITE)]).with_samples(4);
        assert_eq!(square.tolerance(), Tolerance::MULTISAMPLED);
    }
}
