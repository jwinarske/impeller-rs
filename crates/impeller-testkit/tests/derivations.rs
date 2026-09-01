//! What the scene model derives about itself, held to the shape of the model.
//!
//! Every check in this file exists because the same mistake has been made four
//! times: a field is added to the scene format, and a derivation that
//! enumerates what it cares about goes on not caring about it. Each time the
//! failure was silent -- a plate reported as covering a feature nothing
//! applied, textures left unbound for a scene that reads them -- and each time
//! it was found by a picture rather than by a test.
//!
//! What is different here is the mechanism. These do not list the fields they
//! expect; they build a value with a full struct literal and no `..default()`,
//! so a new field on the type stops this file compiling until somebody comes
//! here and says which side of the question it falls on. That is the only kind
//! of check that has not already been got past once.
//!
//! `LayerSpec` and not `Item`, and the line is drawn deliberately. A layer's
//! fields are nearly all treatments -- of the group, or of what is behind it --
//! so asking of a new one "is this a feature?" is a question it will usually
//! have an interesting answer to. An item's are mostly geometry, it gains them
//! often, and locking it the same way would stop the build on every field that
//! was never going to be a feature. Three of the four misses were on a layer.

// Reached from helpers rather than test bodies, so clippy's test-code
// exemption does not see them.
#![allow(clippy::panic)]

use impeller_core::{ColorFilter, ImageFilter};
use impeller_hal::BlendMode;
use impeller_testkit::scene::{LayerSpec, MorphologySpec, Node, Transform};
use impeller_testkit::Scene;

/// A layer with every field set to something that is not its default.
///
/// The struct literal is exhaustive on purpose and must stay that way: adding
/// a field to `LayerSpec` should break this line, and the fix is to give the
/// new field a loud value here and then decide, below, whether `plain` takes
/// it away.
fn every_field_loud() -> LayerSpec {
    LayerSpec {
        blur: 3.0,
        matrix: Some(Transform {
            scale: [2.0, 2.0],
            ..Transform::default()
        }),
        alpha: 0.5,
        blend: BlendMode::Multiply,
        filter: ImageFilter::blur(4.0),
        backdrop: ImageFilter::blur(5.0),
        backdrop_blur: 6.0,
        morphology: Some(MorphologySpec {
            radius: [2.0, 3.0],
            dilate: true,
        }),
        color_filter: ColorFilter::linear_to_srgb(),
        backdrop_id: Some(7),
    }
}

fn layer_of(scene: &Scene) -> LayerSpec {
    match scene.items.first() {
        Some(Node::Layer { layer, .. }) => (**layer).clone(),
        other => panic!("expected one layer node, got {other:?}"),
    }
}

/// `plain` takes away everything that is a feature and nothing that is not.
///
/// The expected value is written as the four fields that survive, over a
/// default: so a field `plain` forgets keeps its loud value and fails here,
/// and a field `plain` should not have touched loses one and fails here too.
/// Which of the two it is, the message cannot say and the diff can.
#[test]
fn plain_takes_a_layer_back_to_its_features_and_leaves_the_rest() {
    let loud = every_field_loud();
    let scene = Scene::tree("probe", vec![Node::layer(loud.clone(), Vec::new())]);
    let stripped = layer_of(&scene.plain());

    // Alpha, blend and the layer's matrix are not features in the sense this
    // pair means: they are how the group composites and where its result goes,
    // which is the picture rather than a treatment of it. `backdrop_id` says
    // *which* capture a backdrop filters and not whether one happens, so a
    // scene with its filters gone has nothing for it to change; it is checked
    // by its own pair of plates instead.
    let expected = LayerSpec {
        matrix: loud.matrix,
        alpha: loud.alpha,
        blend: loud.blend,
        backdrop_id: loud.backdrop_id,
        ..LayerSpec::default()
    };
    assert_eq!(stripped, expected);
}

/// And a layer carrying any one of them is reported as carrying a feature.
///
/// One field at a time rather than all at once, because the derivation is a
/// chain of `||` and all-at-once passes as soon as any single term is right.
/// The list is built by taking the loud layer and putting one field back to
/// its default, so it inherits the exhaustiveness above.
#[test]
fn a_layer_carrying_any_one_feature_says_so() {
    let loud = every_field_loud();
    let plain = LayerSpec::default();
    let one_of = [
        (
            "blur",
            LayerSpec {
                blur: loud.blur,
                ..plain.clone()
            },
        ),
        (
            "filter",
            LayerSpec {
                filter: loud.filter.clone(),
                ..plain.clone()
            },
        ),
        (
            "backdrop",
            LayerSpec {
                backdrop: loud.backdrop.clone(),
                ..plain.clone()
            },
        ),
        (
            "backdrop_blur",
            LayerSpec {
                backdrop_blur: loud.backdrop_blur,
                ..plain.clone()
            },
        ),
        (
            "morphology",
            LayerSpec {
                morphology: loud.morphology,
                ..plain.clone()
            },
        ),
        (
            "color_filter",
            LayerSpec {
                color_filter: loud.color_filter,
                ..plain.clone()
            },
        ),
    ];
    for (name, spec) in one_of {
        let scene = Scene::tree("probe", vec![Node::layer(spec, Vec::new())]);
        assert!(
            scene.carries_a_visual_feature(),
            "a layer carrying only `{name}` should read as carrying a feature"
        );
    }

    // And the ones that are not features do not make it say so, which is the
    // half that would fail if the derivation were widened to "anything set".
    for (name, spec) in [
        (
            "matrix",
            LayerSpec {
                matrix: loud.matrix,
                ..plain.clone()
            },
        ),
        (
            "alpha",
            LayerSpec {
                alpha: loud.alpha,
                ..plain.clone()
            },
        ),
        (
            "blend",
            LayerSpec {
                blend: loud.blend,
                ..plain.clone()
            },
        ),
        (
            "backdrop_id",
            LayerSpec {
                backdrop_id: loud.backdrop_id,
                ..plain.clone()
            },
        ),
    ] {
        let scene = Scene::tree("probe", vec![Node::layer(spec, Vec::new())]);
        assert!(
            !scene.carries_a_visual_feature(),
            "`{name}` is not a feature and should not read as one"
        );
    }
}
