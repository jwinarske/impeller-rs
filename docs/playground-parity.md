# The playground, against Impeller's

Impeller's playground is not a separate program. It is a mode of its test
suite: a `TEST_P` that calls `OpenPlaygroundHere` renders one frame, and with
`--enable_playground` set it opens a window instead of running headless. Its
scenes are therefore its tests, grouped by topic across
`impeller/display_list/aiks_dl_*_unittests.cc`.

This document is the inventory. What that suite contains, what is reproduced
here, and what is not — with the reason named, so a row is either work to do or
a decision that has been made rather than an unexplained gap.

## How this one is arranged

Two collections, both data rather than code, so both reach the window and the
test suite from one list.

The **corpus** is small and tuned. Each scene is there because it exercises
something no other scene does, its tolerance is argued about individually, and
it is compared against a software reference as well as across backends. A
failure in the corpus names a capability.

The **catalog** is this document's subject: the same pictures Impeller's
playground offers, as far as this renderer can draw them. Broad rather than
minimal, one tolerance for the whole collection rather than one per scene. A
failure in the catalog names a picture.

Keeping them apart is deliberate. Several hundred scenes organized around
another project's test suite would destroy what makes the corpus useful, and
folding the corpus into the catalog would lose the per-scene rigor.

## The count

Counts are from the files as fetched, and are the number of `TEST_P` cases
rather than of playground scenes — most call `OpenPlaygroundHere`, a few only
assert. Nothing here checks them mechanically: this repository has no copy of
that source, and a number in a document that nothing verifies is a number to
treat as approximate.

| File | Scenes there | Here | Blocked on |
|---|---|---|---|
| `aiks_dl_basic_unittests.cc` | ~85 | 17 | superellipses, perspective, images, subpass optimizations |
| `aiks_dl_path_unittests.cc` | ~31 | 9 | conics, difference-of-rounded-rects |
| `aiks_dl_gradient_unittests.cc` | ~40 | 17 | dithering |
| `aiks_dl_clip_unittests.cc` | ~5 | 3 | difference clips |
| `aiks_dl_opacity_unittests.cc` | ~3 | 2 | subpass collapse |
| `aiks_dl_blend_unittests.cc` | ~21 | 0 | the scene model cannot name a colour filter's blend form yet |
| `aiks_dl_blur_unittests.cc` | ~59 | 0 | mask blur styles: inner, outer and solid |
| `aiks_dl_vertices_unittests.cc` | ~16 | 0 | the scene model cannot describe a mesh |
| `aiks_dl_atlas_unittests.cc` | ~15 | 0 | the scene model cannot carry a texture |
| `aiks_dl_shadow_unittests.cc` | ~30 | 0 | `drawShadow` |
| `aiks_dl_primitive_shape_unittests.cc` | ~2 | 0 | one is a playground harness, one is a hairline skew |
| `aiks_dl_text_unittests.cc` | — | 0 | text shaping and font parsing, out of scope |
| `aiks_dl_runtime_effect_unittests.cc` | — | 0 | runtime effects |
| `aiks_dl_unittests.cc` | ~39 | 0 | mostly internal optimizations and picture round-trips |

The catalog holds forty-eight scenes of roughly three hundred and fifty, and
the proportion is less interesting than which ones: the arithmetic of drawing is largely covered, and
what is missing is either a capability this renderer does not have or a thing
the scene model cannot describe.

## What blocks the rest

Three different kinds of obstacle, worth separating because only one of them is
about the renderer.

**Capabilities this renderer lacks.** `drawShadow` and its elevation rule.
Round superellipses. Runtime effects. Perspective transforms, which the
transform type is affine and two-dimensional by design. Mask blur styles —
inner, outer and solid — where only the normal style exists. Dithering.
Conic path segments. These are the rows of `docs/parity.md`, and the scenes
that need them arrive when the row does.

**Things the scene model cannot describe**, which is a testkit limitation and
not a renderer one. A scene is a list of shapes with fills; it cannot name a
mesh, a texture, or a colour filter stated as a blend against a constant. All
three exist in the renderer and have their own tests — the meshes and the
sprite batches are in the playground's live scenes, and the colour filters are
in the public API's suite. Extending the scene model would bring roughly thirty
more scenes across, and is the cheapest remaining tranche.

**Things that are deliberately out of scope.** Text, which needs shaping and
font parsing that `docs/architecture.md` places outside this project. And the
tests that check an optimization rather than a picture — subpass collapse,
clear-colour elision, peepholes — which assert about how a frame was rendered
rather than what it looks like, and which this renderer does not implement the
optimizations for.

## The scenes

Named for the test each mirrors, in the form `topic/CppTestName` reduced to
kebab case, so the original can be found by its name and a scene here with no
counterpart there would be visible as one. `catalog()` in
`crates/impeller-testkit/src/catalog.rs` is the list; the playground shows it
after the corpus, and `cargo test -p impeller-testkit --test catalog` renders
every one of them on both backends and compares.
