//! What the public API does with data a caller did not sanitize.
//!
//! `impeller-geometry`'s `hostile.rs` covers the path and the tessellator,
//! which is where the geometry arrives. This covers the rest of the front
//! door: the calls that take a caller's *arrays* -- a mesh's positions and
//! indices, an atlas's sprites and source rectangles, a glyph run's placements,
//! a gradient's stops. Those are the arguments most likely to come from a file
//! or a wire rather than from a literal, and each has a shape the renderer
//! relies on.
//!
//! Recording only, with no device. Everything asserted here is decided while
//! building a `Recording`, so the whole file runs anywhere and in a
//! millisecond -- and a defect in it is a defect on every backend rather than
//! on the one that happened to be present.
//!
//! The claim is the same one the geometry file makes and is worth repeating
//! because it is easy to weaken by accident: *garbage in may give garbage out,
//! but it must not give a panic, and it must not give a recording that names
//! something that is not there.* A draw whose vertex indices run off the end
//! of its buffer is not a wrong picture; it is a read the driver performs on
//! this process's behalf.
//!
//! # These passed the first time they were run, which is not evidence
//!
//! All four did, and a property test that has never failed has not been shown
//! to be able to. So the mesh one was checked against the guard it is about:
//! deleting the index bounds check in `Vertices::build` did *not* fail it, and
//! the reason turned out to be worth knowing rather than a flaw. The invariant
//! has two independent guards -- `Vertices` refuses the index, and
//! `Batch::push` refuses it again with its own message -- so removing either
//! leaves the other holding, and the recording stays addressable. Removing
//! both fails the property, on `index 0 against 0 vertices`.
//!
//! That is the shape of thing worth writing down: the property is sound, the
//! codebase is belt-and-braces here, and a mutation that removes one belt
//! proves nothing about the test.

use impeller::*;
use proptest::prelude::*;

/// Floats chosen to break arithmetic rather than to describe a picture.
fn hostile_coord() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -500.0f32..500.0,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => Just(0.0f32),
        1 => Just(-0.0f32),
        1 => Just(f32::MIN_POSITIVE),
        1 => Just(f32::MAX),
        1 => Just(1e20f32),
        1 => Just(-1e20f32),
    ]
}

fn hostile_point() -> impl Strategy<Value = [f32; 2]> {
    (hostile_coord(), hostile_coord()).prop_map(|(x, y)| [x, y])
}

fn hostile_color() -> impl Strategy<Value = Color> {
    (
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
    )
        .prop_map(|(r, g, b, a)| Color::srgb(r, g, b, a))
}

/// Every draw in every pass names vertices that exist, and every texture slot
/// it samples was declared by the pass it sits in.
///
/// The one invariant that is not about the picture. Everything below builds a
/// recording out of hostile input and then asks this of it.
fn recording_is_addressable(recording: &Recording) -> Result<(), String> {
    for (n, pass) in recording.passes.iter().enumerate() {
        let vertices = pass.batch.vertices().len();
        for index in pass.batch.indices() {
            if *index as usize >= vertices {
                return Err(format!(
                    "pass {n}: index {index} against {vertices} vertices"
                ));
            }
        }
        if pass.batch.indices().len() % 3 != 0 {
            return Err(format!(
                "pass {n}: {} indices is not whole triangles",
                pass.batch.indices().len()
            ));
        }
        for slot in pass.batch.texture_slots() {
            if slot as usize >= pass.sources.len() {
                return Err(format!(
                    "pass {n}: slot {slot} against {} sources",
                    pass.sources.len()
                ));
            }
        }
    }
    Ok(())
}

const SIZE: Extent2D = Extent2D {
    width: 128,
    height: 128,
};

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// A mesh built from arbitrary positions, indices and colors.
    ///
    /// `Vertices` refuses an index that names a position it does not have, and
    /// that refusal is the thing under test as much as the drawing is: this
    /// asks for the refusal by generating indices against a shorter list than
    /// they address, over and over.
    #[test]
    fn a_mesh_of_any_shape_records_something_addressable(
        positions in prop::collection::vec(hostile_point(), 0..12),
        indices in prop::collection::vec(0u32..16, 0..24),
        colors in prop::collection::vec(hostile_color(), 0..12),
    ) {
        let positions: Vec<Vec2> = positions.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        for mode in [VertexMode::Triangles, VertexMode::TriangleStrip, VertexMode::TriangleFan] {
            let mesh = Vertices::indexed(mode, positions.clone(), Vec::new(), indices.clone());
            let _ = &colors;
            let Ok(mesh) = mesh else { continue };
            let mut canvas = Canvas::new(SIZE);
            canvas.clear(Color::BLACK);
            let _ = canvas.draw_vertices(&mesh, &Paint::fill(Color::WHITE));
            let recording = canvas.finish();
            prop_assert!(
                recording_is_addressable(&recording).is_ok(),
                "{:?}",
                recording_is_addressable(&recording)
            );
        }
    }

    /// A gradient whose stops are in no particular order, at no particular
    /// offsets, in any number.
    ///
    /// Offsets are what the shader walks to find a color, and a walk over
    /// unsorted or non-finite offsets is the sort of thing that reads past the
    /// end of a stop table. More than `MAX_STOPS` takes the tabulated route
    /// instead, which is a second code path with the same question.
    #[test]
    fn a_gradient_of_any_stops_records_something_addressable(
        stops in prop::collection::vec((hostile_color(), hostile_coord()), 0..12),
        start in hostile_point(),
        end in hostile_point(),
    ) {
        let stops: Vec<GradientStop> = stops
            .into_iter()
            .map(|(color, offset)| GradientStop::new(color, offset))
            .collect();
        let paint = Paint::default().with_shader(Shader::LinearGradient {
            start: Vec2::new(start[0], start[1]),
            end: Vec2::new(end[0], end[1]),
            stops,
            tile: TileMode::Clamp,
        });
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 120.0, 120.0), &paint);
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{:?}",
            recording_is_addressable(&recording)
        );
    }

    /// A run of glyphs placed anywhere, at any size, naming any atlas cell.
    #[test]
    fn a_glyph_run_of_any_placement_records_something_addressable(
        placements in prop::collection::vec((hostile_point(), hostile_point(), 0u32..64), 0..10),
    ) {
        let glyphs: Vec<PositionedGlyph> = placements
            .into_iter()
            .map(|(position, size, id)| PositionedGlyph {
                key: GlyphKey { font: 0, glyph: id as u16, size: 12 },
                position,
                size,
            })
            .collect();
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        // An empty atlas, which is the hostile case for a run: every glyph
        // names a cell the atlas does not hold.
        let atlas = Atlas::new(64);
        let _ = canvas.draw_glyphs(&glyphs, &atlas, 0, &Paint::fill(Color::WHITE));
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{:?}",
            recording_is_addressable(&recording)
        );
    }

    /// Sprites whose source rectangles need not lie inside any image.
    #[test]
    fn an_atlas_of_any_sprites_records_something_addressable(
        sprites in prop::collection::vec(
            (hostile_coord(), hostile_coord(), hostile_coord(), hostile_coord(), hostile_point()),
            0..10,
        ),
    ) {
        let sprites: Vec<Sprite> = sprites
            .into_iter()
            .map(|(x, y, w, h, at)| Sprite {
                source: SourceRect::new(x, y, w, h),
                color: Color::WHITE,
                transform: Affine2::from_translation(Vec2::new(at[0], at[1])),
            })
            .collect();
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let _ = canvas.draw_atlas(&sprites, SIZE, &Paint::fill(Color::WHITE));
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{:?}",
            recording_is_addressable(&recording)
        );
    }
}

/// A stack of layers deeper than any interface needs is recorded, not refused and
/// not fatal.
///
/// `save_layer` is the one call a caller can nest without bound, and each nesting
/// costs a pass and a target for it. So the arithmetic a hostile caller reaches is not
/// a big array but a deep stack, and the two failures worth ruling out are a recursion
/// that runs out of stack and a limit that was never written down.
///
/// Recorded here rather than executed, like the rest of this file, and the numbers
/// below say why that is not a dodge: a thousand nested layers at sixty-four pixels
/// square draw in a tenth of a second, and what a device would then do with a
/// thousand targets is a question about memory on that device rather than about this
/// API. The recording is where a stack overflow would happen, and it does not.
///
/// What this does not claim is that a deep nest is *useful*. Each layer composites
/// through its own opacity, so a thousand at nine tenths each leaves the drawing
/// multiplied by nine tenths to the thousandth, which is nothing. Arriving at
/// nothing by arithmetic is a correct picture; arriving at it by aborting is not.
#[test]
fn a_stack_of_layers_deeper_than_anything_needs_is_still_a_recording() {
    for depth in [1usize, 64, 1024] {
        let mut canvas = Canvas::new(Extent2D::new(64, 64));
        canvas.clear(Color::BLACK);
        for _ in 0..depth {
            canvas.save_layer(Layer::opacity(0.9));
        }
        assert_eq!(
            canvas.layer_depth(),
            depth,
            "the canvas lost count of the layers it has open"
        );
        canvas
            .draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE))
            .expect("a rectangle inside a deep stack is still a rectangle");
        for _ in 0..depth {
            canvas.restore();
        }
        assert_eq!(canvas.layer_depth(), 0, "a balanced stack did not unwind");

        // One pass per layer and one for the root, which is what says the nesting
        // reached the recording rather than being flattened away somewhere.
        let recording = canvas.finish();
        assert_eq!(
            recording.passes.len(),
            depth + 1,
            "a nest of {depth} produced {} passes",
            recording.passes.len()
        );
    }
}

/// Unbalanced nesting neither panics nor invents a pass.
///
/// A caller who opens layers and forgets to close them is the ordinary way a deep
/// stack arrives -- a loop with an early return in it. `finish` has to cope, and what
/// it must not do is drop the drawing on the floor without saying so.
#[test]
fn layers_left_open_are_finished_rather_than_lost() {
    let mut canvas = Canvas::new(Extent2D::new(64, 64));
    canvas.clear(Color::BLACK);
    for _ in 0..32 {
        canvas.save_layer(Layer::opacity(0.9));
    }
    canvas
        .draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE))
        .expect("a rectangle");
    assert_eq!(canvas.layer_depth(), 32, "the layers did not open");

    // No `restore` at all: thirty-two layers still open when the recording is asked
    // for.
    let recording = canvas.finish();
    assert_eq!(
        recording.passes.len(),
        33,
        "an unbalanced stack lost the passes its layers opened"
    );
    assert!(
        recording.draw_count() > 0,
        "the drawing inside an unclosed layer went nowhere"
    );
}

/// Every field of a [`Layer`], generated.
///
/// Written as a struct literal on purpose, and that is the whole reason this is a
/// function rather than a chain of `with_` calls. `Layer` has eight fields and is
/// not `#[non_exhaustive]`, so a literal makes adding a ninth a compile error here
/// -- which is the only mechanism that keeps a generator honest as a type grows.
/// A builder chain would keep compiling and would silently stop covering the new
/// field, and this is a type that has grown twice: `morphology` and
/// `backdrop_blur` both arrived after the first pass over it, and both were missed
/// by an enumeration written out by hand.
fn hostile_layer() -> impl Strategy<Value = Layer> {
    (
        // Grouped in threes because `prop_oneof!` and tuple strategies both stop
        // at twelve, and eight fields plus their inner parts exceed it.
        (hostile_coord(), hostile_coord(), hostile_coord()),
        (
            prop::sample::select(BlendMode::ALL),
            prop::option::of((hostile_coord(), hostile_coord(), hostile_coord())),
            prop::option::of((hostile_coord(), hostile_coord(), any::<bool>())),
        ),
        (
            prop_oneof![
                4 => Just(ColorFilter::None),
                1 => (hostile_coord(), prop::sample::select(BlendMode::ALL))
                    .prop_map(|(c, mode)| ColorFilter::Blend {
                        color: [c, c, c, c],
                        mode,
                    }),
                1 => Just(ColorFilter::Gamma { direction: Gamma::LinearToSrgb }),
                1 => Just(ColorFilter::Gamma { direction: Gamma::SrgbToLinear }),
            ],
            prop::option::of(any::<i64>()),
        ),
    )
        .prop_map(
            |(
                (blur_x, blur_y, alpha),
                (blend, matrix, morphology),
                (color_filter, backdrop_id),
            )| Layer {
                blur: Vec2::new(blur_x, blur_y),
                alpha,
                blend,
                matrix: matrix.map(|(x, y, r)| {
                    Transform2D::from(Affine2::from_scale(Vec2::new(x, y)))
                        * Transform2D::from(Affine2::from_angle(r))
                }),
                backdrop_blur: blur_y,
                backdrop_id,
                morphology: morphology.map(|(x, y, dilate)| Morphology {
                    radius: [x, y],
                    dilate,
                }),
                color_filter,
            },
        )
}

/// One call a caller could make, in a sequence of them.
///
/// The arrays a draw carries are covered above. This is the other axis and the one
/// nothing reached: the *order* calls arrive in, and in particular a stack that
/// does not balance. `Restore` is generated freely, including more often than
/// `Save`, because restoring past the bottom is a thing a caller does and must not
/// be a panic.
#[derive(Debug, Clone)]
enum Op {
    Save,
    Restore,
    SaveLayer(Layer),
    SaveLayerBounds(Layer, [f32; 4]),
    ClipRect([f32; 4]),
    ClipOutRect([f32; 4]),
    Translate(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
    DrawRect([f32; 4], Color),
    DrawCircle([f32; 2], f32, Color),
    DrawPaint(Color),
    Clear(Color),
}

fn hostile_rect() -> impl Strategy<Value = [f32; 4]> {
    (
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
    )
        .prop_map(|(a, b, c, d)| [a, b, c, d])
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        3 => Just(Op::Save),
        4 => Just(Op::Restore),
        3 => hostile_layer().prop_map(Op::SaveLayer),
        2 => (hostile_layer(), hostile_rect())
            .prop_map(|(l, r)| Op::SaveLayerBounds(l, r)),
        2 => hostile_rect().prop_map(Op::ClipRect),
        1 => hostile_rect().prop_map(Op::ClipOutRect),
        2 => (hostile_coord(), hostile_coord()).prop_map(|(x, y)| Op::Translate(x, y)),
        2 => (hostile_coord(), hostile_coord()).prop_map(|(x, y)| Op::Scale(x, y)),
        1 => hostile_coord().prop_map(Op::Rotate),
        4 => (hostile_rect(), hostile_color()).prop_map(|(r, c)| Op::DrawRect(r, c)),
        2 => (hostile_point(), hostile_coord(), hostile_color())
            .prop_map(|(at, radius, c)| Op::DrawCircle(at, radius, c)),
        1 => hostile_color().prop_map(Op::DrawPaint),
        1 => hostile_color().prop_map(Op::Clear),
    ];
    prop::collection::vec(op, 0..40)
}

/// Apply one operation, ignoring the errors a hostile argument earns.
///
/// A refusal is a correct outcome here and is not what is under test -- the
/// invariants below are about what the canvas is left holding, which has to be
/// sound whether a call was taken or declined.
fn apply(canvas: &mut Canvas, op: &Op) {
    let rect = |r: &[f32; 4]| Rect::new(r[0], r[1], r[2], r[3]);
    match op {
        Op::Save => {
            canvas.save();
        }
        Op::Restore => {
            canvas.restore();
        }
        Op::SaveLayer(layer) => {
            canvas.save_layer(*layer);
        }
        Op::SaveLayerBounds(layer, r) => {
            canvas.save_layer_bounds(*layer, rect(r));
        }
        Op::ClipRect(r) => {
            let _ = canvas.clip_rect(rect(r));
        }
        Op::ClipOutRect(r) => {
            let _ = canvas.clip_out_rect(rect(r));
        }
        Op::Translate(x, y) => {
            canvas.translate(*x, *y);
        }
        Op::Scale(x, y) => {
            canvas.scale(*x, *y);
        }
        Op::Rotate(r) => {
            canvas.rotate(*r);
        }
        Op::DrawRect(r, color) => {
            let _ = canvas.draw_rect(rect(r), &Paint::fill(*color));
        }
        Op::DrawCircle(at, radius, color) => {
            let _ = canvas.draw_circle(Vec2::new(at[0], at[1]), *radius, &Paint::fill(*color));
        }
        Op::DrawPaint(color) => {
            let _ = canvas.draw_paint(&Paint::fill(*color));
        }
        Op::Clear(color) => {
            canvas.clear(*color);
        }
    }
}

/// How many of a sequence's operations open a stack entry.
fn opens(ops: &[Op]) -> usize {
    ops.iter()
        .filter(|op| matches!(op, Op::Save | Op::SaveLayer(_) | Op::SaveLayerBounds(..)))
        .count()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// Any sequence of calls, in any order, leaves a recording that names only
    /// what it holds.
    ///
    /// The same invariant the four properties above assert, reached along the axis
    /// they do not touch. Each of those makes one well-formed call carrying a
    /// hostile array; this makes up to forty calls whose *order* is hostile --
    /// unbalanced restores, clips under a degenerate transform, layers left open
    /// at `finish`.
    #[test]
    fn a_sequence_of_any_operations_records_something_addressable(ops in ops()) {
        let mut canvas = Canvas::new(SIZE);
        for op in &ops {
            apply(&mut canvas, op);
            // Checked inside the loop rather than only at the end, because the
            // two counts are a property of every intermediate state and a
            // violation the next operation repairs would otherwise go unseen.
            prop_assert!(
                canvas.layer_depth() <= canvas.save_depth(),
                "{} layers open against {} saves, so a layer is not on the stack \
                 it is counted from",
                canvas.layer_depth(),
                canvas.save_depth()
            );
            prop_assert!(
                canvas.save_depth() <= opens(&ops),
                "{} saves outstanding after at most {} operations that open one",
                canvas.save_depth(),
                opens(&ops)
            );
        }
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{}",
            recording_is_addressable(&recording).unwrap_err()
        );
    }

    /// The stack unwinds in exactly as many restores as it says are outstanding.
    ///
    /// `save_depth` is documented as being for "a caller checking its own
    /// balance", which is only true if it is the number of restores that balance
    /// it. That makes it a round trip rather than a reading: ask, restore that
    /// many times, and nothing may be left -- including no open layer, since
    /// `restore` pops whichever kind of entry is on top and a layer left behind
    /// would mean the two counts disagree about the same stack.
    #[test]
    fn the_stack_unwinds_in_as_many_restores_as_it_says(ops in ops()) {
        let mut canvas = Canvas::new(SIZE);
        for op in &ops {
            apply(&mut canvas, op);
        }
        let outstanding = canvas.save_depth();
        for _ in 0..outstanding {
            canvas.restore();
        }
        prop_assert_eq!(
            canvas.save_depth(),
            0,
            "{} restores left the stack un-emptied",
            outstanding
        );
        prop_assert_eq!(
            canvas.layer_depth(),
            0,
            "the saves are balanced but {} layers are still open",
            canvas.layer_depth()
        );
        // And one more past the bottom is a no-op rather than a panic, which is
        // what a caller over-restoring in an error path does.
        canvas.restore();
        prop_assert_eq!(canvas.save_depth(), 0);
    }

    /// A layer configured any way at all still records addressably.
    ///
    /// Reached through `save_layer` directly rather than through a sequence, so a
    /// failure names the layer rather than the forty calls around it. Every field
    /// is generated, including the two an earlier hand-written enumeration of this
    /// type missed.
    #[test]
    fn a_layer_of_any_configuration_records_something_addressable(
        layer in hostile_layer(),
        bounds in prop::option::of(hostile_rect()),
    ) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        match bounds {
            Some(r) => {
                canvas.save_layer_bounds(layer, Rect::new(r[0], r[1], r[2], r[3]));
            }
            None => {
                canvas.save_layer(layer);
            }
        }
        let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE));
        canvas.restore();
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{}",
            recording_is_addressable(&recording).unwrap_err()
        );
    }
}

/// The sequences reach the states the properties above are about.
///
/// Three of them could pass on a generator that never opened a layer, never left
/// one open at `finish`, and never restored past the bottom -- the three states
/// that distinguish this from the single-call properties. So this counts them.
/// Written as an ordinary test because it is a claim about the generator.
#[test]
fn the_generated_sequences_reach_the_states_worth_reaching() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = ops();
    let mut opened = 0usize;
    let mut left_open = 0usize;
    let mut over_restored = 0usize;
    let mut drew = 0usize;
    const TRIES: usize = 512;
    for _ in 0..TRIES {
        let ops = strategy
            .new_tree(&mut runner)
            .expect("the generator above is infallible")
            .current();
        let mut canvas = Canvas::new(SIZE);
        let mut bottomed = false;
        for op in &ops {
            if matches!(op, Op::Restore) && canvas.save_depth() == 0 {
                bottomed = true;
            }
            apply(&mut canvas, op);
        }
        if ops
            .iter()
            .any(|op| matches!(op, Op::SaveLayer(_) | Op::SaveLayerBounds(..)))
        {
            opened += 1;
        }
        if canvas.layer_depth() > 0 {
            left_open += 1;
        }
        if bottomed {
            over_restored += 1;
        }
        if canvas.finish().draw_count() > 0 {
            drew += 1;
        }
    }
    for (what, count) in [
        ("opened a layer", opened),
        ("left a layer open at finish", left_open),
        ("restored past the bottom", over_restored),
        ("recorded a draw", drew),
    ] {
        assert!(
            count * 10 >= TRIES,
            "only {count} of {TRIES} generated sequences {what}, so the \
             properties over that state are close to vacuous"
        );
    }
}
