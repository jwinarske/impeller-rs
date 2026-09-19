//! The frames the bench measures: what is drawn, how much of it, and how many
//! times.
//!
//! Split from `bench.rs` so the drift counter can watch what is *measured* without
//! flagging every change to how it is reported or tested. That counter exists
//! because a commit can move a recorded number with nothing in the tree saying so,
//! and it proved the point against itself on 2026-09-18: with `bench.rs` watched
//! whole, the next two commits were a test and the counter's own fix, and both were
//! reported as having touched what the bench times. Two false positives out of two
//! is how a printed warning becomes one nobody reads.
//!
//! So the rule for this file is narrow and worth stating. Anything here changes what
//! a recorded number means, and a change here wants a board run behind it. Anything
//! in `bench.rs` -- the clock, the report, the baseline parsing, the tests -- does
//! not.

use impeller_core::{
    Canvas, Color, GradientStop, Layer, Paint, Recording, Rect, Shader, TileMode, Vec2,
};
use impeller_hal::Extent2D;

/// The frame the document's number was taken from.
pub(super) const EXTENT: Extent2D = Extent2D {
    width: 1920,
    height: 1080,
};

/// A hundred and sixty rounded rectangles, as the document says.
pub(super) const SHAPES: usize = 160;

/// Rendered before timing starts, so pipeline creation and the first
/// allocation of every buffer are not counted as frame cost.
pub(super) const WARMUP: usize = 5;

/// Timed frames.
///
/// Two hundred rather than the thirty this began with, and the reason is the
/// percentile below. A ninety-ninth percentile of thirty samples is the
/// largest of them by another name -- `ceil(0.99 * 30)` is thirty -- so
/// reporting one would have dressed the maximum up as a distribution. Two
/// hundred puts the ninety-ninth at the third-largest, which is a tail rather
/// than an outlier, and costs about fifty milliseconds a configuration at the
/// times this actually measures.
pub(super) const FRAMES: usize = 200;

/// The percentile below is only a percentile if there are samples enough for it.
///
/// At the compiler rather than in a test, because it is a statement about a
/// constant: `ceil(0.99 * n)` is `n` for every `n` under a hundred, so a p99
/// taken from fewer would be the maximum under another name and no run would
/// say so.
const _: () = assert!(FRAMES >= 100);

/// How a frame's shapes are drawn, which is the whole question.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// A rounded rectangle stated as one, which reaches the analytic distance
    /// field: coverage from an implicit function in the fragment stage, over a
    /// quad, with the pass left at one sample.
    Analytic,
    /// The same shape stated as a path, which tessellates it. Antialiased, so
    /// the pass multisamples.
    TessellatedMultisampled,
    /// The same again with antialiasing off, which leaves the pass at one
    /// sample and is the third of the document's three figures.
    TessellatedSingleSampled,
    /// The same shape *stroked*, through the analytic field: a distance to the
    /// outline rather than to the interior, still one sample.
    ///
    /// Strokes were timed by nothing at all until this, which is how a shader
    /// change that cost two and a half per cent reached a release. They are
    /// their own pair of routes and deserve their own comparison: what an
    /// outline costs is not what a fill costs, and the tessellated one has to
    /// build two contours where a fill builds one.
    StrokedAnalytic,
    /// The stroked shape as a path, which sends it through the stroker.
    /// Antialiasing off, so this and the field above are one sample each and
    /// the difference between them is the route.
    StrokedTessellated,
}

impl Path {
    pub fn name(self) -> &'static str {
        match self {
            Self::Analytic => "distance field, 1 sample",
            Self::TessellatedMultisampled => "tessellated, 4 samples",
            Self::TessellatedSingleSampled => "tessellated, 1 sample",
            Self::StrokedAnalytic => "stroked field, 1 sample",
            Self::StrokedTessellated => "stroked path, 1 sample",
        }
    }

    /// Whether this path strokes rather than fills.
    fn strokes(self) -> bool {
        matches!(self, Self::StrokedAnalytic | Self::StrokedTessellated)
    }

    /// Whether the shape goes to the tessellator rather than the field.
    fn tessellates(self) -> bool {
        matches!(
            self,
            Self::TessellatedMultisampled
                | Self::TessellatedSingleSampled
                | Self::StrokedTessellated
        )
    }
}

/// Wide enough that the stroke is geometry rather than the thin-stroke rule.
///
/// A width under a device pixel is widened to one and dimmed to pay for it,
/// which is a different measurement and a much smaller one -- what these two
/// rows are for is what an outline costs when there is an outline. Four device
/// pixels at this frame size, on shapes a hundred and twenty across.
pub(super) const STROKE_WIDTH: f32 = 4.0;

/// Under a device pixel the width is widened to one and dimmed to pay for it,
/// which is a different measurement and a much smaller one. Held here so that
/// lowering the constant fails the build rather than quietly changing what the
/// two rows mean.
const _: () = assert!(STROKE_WIDTH >= 1.0);

/// Lay the shapes out in a grid that fills the frame.
///
/// Placed off the whole pixel deliberately: an axis-aligned rectangle at
/// integer bounds has no edge to antialias, which is the one case where the
/// field's advantage does not exist and the comparison would flatter it.
pub(super) fn shapes() -> impl Iterator<Item = Rect> {
    let columns = 16usize;
    let rows = SHAPES / columns;
    let width = EXTENT.width as f32 / columns as f32;
    let height = EXTENT.height as f32 / rows as f32;
    (0..SHAPES).map(move |i| {
        let (column, row) = (i % columns, i / columns);
        let left = column as f32 * width + 4.3;
        let top = row as f32 * height + 4.7;
        Rect::new(left, top, left + width - 8.0, top + height - 8.0)
    })
}

/// One frame's worth of drawing, recorded the way the path under test asks.
pub fn recording(path: Path) -> Recording {
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::BLACK);
    // The field route needs antialiasing -- its coverage *is* the distance, and
    // the analytic path declines a paint that asked for none -- while the
    // tessellated rows turn it off to stay at one sample. So this is which
    // route the path names rather than a per-path flag.
    let anti_alias = !matches!(
        path,
        Path::TessellatedSingleSampled | Path::StrokedTessellated
    );
    let paint = match path.strokes() {
        true => Paint::stroke(Color::WHITE, STROKE_WIDTH),
        false => Paint::fill(Color::WHITE),
    }
    .with_anti_alias(anti_alias);
    for rect in shapes() {
        let drawn = match path.tessellates() {
            // The same shape and the same paint, stated as a path so that the
            // tessellator sees it rather than the fragment stage. Comparing
            // the two forms of one shape is what makes this a measurement of
            // the path rather than of the content.
            true => canvas.draw_path(&rect.to_rounded_path(12.0), &paint),
            false => canvas.draw_rrect(rect, 12.0, &paint),
        };
        drawn.expect("a rounded rectangle");
    }
    canvas.finish()
}

/// How the full-frame row names itself.
pub(super) const FRAME: &str = "full frame, mixed content";

/// What a whole frame of mixed content costs, which is a budget rather than a
/// comparison.
///
/// The three routes above answer one narrow question and answer it well: the
/// same shapes, twice, so the difference is the route. That is not a frame. It
/// has one material, no layer, no blur and no gradient, so it says nothing
/// about what an interface costs — and a renderer can be quick at a hundred
/// and sixty identical rectangles and slow at everything a real frame is made
/// of.
///
/// So this is the other kind: a ground that is a gradient, cards that carry
/// shadows, and a blurred layer over the top, at the size a display actually
/// is. Every one of those reaches machinery the comparison never touches — the
/// ramp, the blur's passes, the layer's own target and its composite back.
///
/// Deliberately *not* the panel example's frame, which it otherwise resembles.
/// That one turns antialiasing off on its ground because `execute_deferred`
/// cannot submit a multisampled pass, which is a constraint of presenting to a
/// display and not of drawing. A frame written to be timed should look like a
/// frame, so this leaves it on.
///
/// Static, at one instant of that scene rather than a moving one: a benchmark
/// that changed its own content between runs would report the content.
pub(super) fn full_frame() -> Recording {
    let (w, h) = (EXTENT.width as f32, EXTENT.height as f32);
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::srgb(0.05, 0.06, 0.09, 1.0));

    // A wash behind everything. A gradient rather than a flat fill because it
    // is the one thing here that tabulates a ramp.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, w, h),
            &Paint::default().with_shader(Shader::LinearGradient {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(w, h),
                // Five, and the count is the point rather than the picture:
                // at or below `MAX_STOPS` a gradient travels inside the
                // material and tabulates nothing. Past it the recorder bakes a
                // ramp and the shader samples it, which is the path a frame
                // should be timed on and the one two stops would miss.
                stops: (0..5)
                    .map(|i| {
                        let t = i as f32 / 4.0;
                        GradientStop {
                            offset: t,
                            color: Color::srgb(0.08 + 0.14 * t, 0.10, 0.18 + 0.02 * t, 1.0),
                        }
                    })
                    .collect(),
                tile: TileMode::Clamp,
            }),
        )
        .expect("the ground");

    let center = Vec2::new(w * 0.5, h * 0.5);
    let orbit = w.min(h) * 0.26;
    let side = w.min(h) * 0.20;
    let hues = [
        Color::srgb(0.98, 0.42, 0.28, 1.0),
        Color::srgb(0.36, 0.82, 0.62, 1.0),
        Color::srgb(0.42, 0.58, 0.98, 1.0),
    ];
    for (i, hue) in hues.into_iter().enumerate() {
        let phase = i as f32 * std::f32::consts::TAU / 3.0;
        let at = Vec2::new(
            center.x + orbit * phase.cos(),
            center.y + orbit * phase.sin() * 0.55,
        );
        let card = Rect::new(
            at.x - side * 0.5,
            at.y - side * 0.5,
            at.x + side * 0.5,
            at.y + side * 0.5,
        );
        // Shadow first and card over it, which is the order every real caller
        // uses and the one the occluder flag describes.
        canvas
            .draw_shadow(&card.to_rounded_path(side * 0.18), Color::BLACK, 8.0, false)
            .expect("a shadow");
        canvas
            .draw_rrect(card, side * 0.18, &Paint::fill(hue))
            .expect("a card");
    }

    // A blurred highlight, so the layer's own target and its composite back are
    // in the number too.
    canvas.save_layer(Layer::opacity(0.5).with_blur(24.0));
    canvas
        .draw_circle(
            center,
            w.min(h) * 0.10,
            &Paint::fill(Color::srgb(1.0, 0.95, 0.80, 1.0)),
        )
        .expect("a highlight");
    canvas.restore();

    canvas.finish()
}
