//! What space a morphology radius is in, pinned.
//!
//! `dilate` and `erode` take a radius, and the two renderers disagree about what
//! it measures. Here it is device pixels and nothing scales it. Upstream's is a
//! local length that `entity.GetTransform() * effect_transform.Basis()` scales at
//! the pass, so a dilated layer under a scale of three spreads three times as
//! far there and the same distance here. `docs/non-parity.md` 17 is the entry,
//! with what a fix costs and why it is not the multiplication it looks like.
//!
//! This file exists so that flipping the convention is *visible*. The corpus has
//! a pair of scenes built to differ in nothing but the transform -- the same
//! cross at half the size under a scale of two, landing on exactly the pixels the
//! unscaled one covers -- so the dilation distance is the only thing left that
//! can separate them.
//!
//! # Why it is not the cost baseline that catches this
//!
//! It looks as though it should be, and it is not, which is worth writing down
//! before someone relies on it. The two scenes record *identical* cost rows: four
//! passes, four draws, twenty vertices, thirty indices, three sources. A pass is
//! emitted per `MORPHOLOGY_TAPS` texels and that constant is thirty-two, so a
//! radius of eight and a radius of sixteen both fit in one pass each way and the
//! pass count does not move. The cost table counts passes and does not read what
//! they carry.
//!
//! So the radius has to be read out of the material, which is what this does. The
//! same reasoning says the two scenes are very likely pixel-identical as well, so
//! the corpus image comparison gains nothing from the pair either -- the second
//! scene earns its place as this pin and as coverage of a filter under a
//! transform, not as another picture.
//!
//! Recording only, so it runs with no device and on every merge.

use impeller_hal::Material;
use impeller_testkit::{corpus, record_scene};

/// The morphology radii a scene's recording ends up carrying, in pass order.
fn radii(name: &str) -> Vec<[f32; 2]> {
    // `ok_or` then `expect` rather than `unwrap_or_else` with a `panic!`: the
    // workspace denies `panic` and allows `expect`, and carrying the name through
    // as the error keeps it in the message.
    let scene = corpus()
        .into_iter()
        .find(|scene| scene.name == name)
        .ok_or(name)
        .expect("the corpus has no scene by this name");
    let recording = record_scene(&scene).expect("a corpus scene records");
    recording
        .passes
        .iter()
        .flat_map(|pass| pass.batch.draws())
        .filter_map(|draw| match draw.material {
            // `step` is the direction and reciprocal extent, `radius` the
            // distance along it. Both are needed: a pass that ran the same
            // distance along both axes would be indistinguishable from one that
            // ran the right distance if only the radius were read.
            Material::Morphology { step, radius, .. } => {
                Some([step[0], step[1]].map(|along| if along == 0.0 { 0.0 } else { radius }))
            }
            _ => None,
        })
        .collect()
}

/// A dilation reaches the same distance whether or not its layer is scaled.
///
/// The current convention, and a divergence from upstream rather than a
/// preference -- see this file's header and `docs/non-parity.md` 17.
///
/// **This test is meant to fail when the convention is flipped.** It is not
/// asserting the better of two behaviors; it is making the worse one impossible
/// to change by accident. Whoever makes the radius local should expect this to
/// go red, and should replace it with the opposite assertion -- the scaled
/// scene's radii doubled -- rather than delete it.
#[test]
fn the_dilation_under_a_scale_reaches_the_same_distance() {
    let unscaled = radii("layer-dilated");
    let scaled = radii("layer-dilated-under-scale");

    assert!(
        !unscaled.is_empty(),
        "no morphology material in layer-dilated, so this compared nothing"
    );
    assert_eq!(
        unscaled, scaled,
        "the dilation reached a different distance under a scale of two. If the \
         radius was deliberately made a local length, this test is what says so \
         -- assert the doubled radii here and update docs/non-parity.md 17, which \
         records the convention as it was."
    );

    // And the radii are the ones the scene asked for, so the equality above is
    // two scenes agreeing on the right answer rather than on nothing. The scene
    // states eight and three, which the shader reads one axis at a time.
    let mut reached: Vec<f32> = unscaled
        .iter()
        .flat_map(|pair| pair.iter().copied())
        .filter(|distance| *distance > 0.0)
        .collect();
    reached.sort_by(f32::total_cmp);
    assert_eq!(
        reached,
        vec![3.0, 8.0],
        "the scene asks for radii of eight and three, and the recording carries \
         {reached:?}"
    );
}
