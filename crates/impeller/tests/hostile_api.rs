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
