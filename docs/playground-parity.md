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
assert. The blend count includes the cases a macro generates per blend mode,
which are the majority of that file. Nothing here checks any of them
mechanically: this repository has no copy of that source, and a number in a
document that nothing verifies is a number to treat as approximate.

The one number describing *this* repository is checked. A scene added to the
catalog without the count following it fails a test, because an inventory that
drifts from what it inventories is worse than none.

The last column says what stops the rest, and it is worth reading with some
suspicion: an entry there is a claim about this renderer, and a claim nobody
tests goes stale quietly. Two of them just did. Blurs under rotation and
clipping together were listed as blocked and were merely unwritten -- both
render, and both agree across the backends. Mipmapped cases were genuinely
blocked and are not any more.

| File | Scenes there | Here | Blocked on |
|---|---|---|---|
| `aiks_dl_basic_unittests.cc` | ~85 | 33 | superellipses, perspective, subpass optimizations |
| `aiks_dl_path_unittests.cc` | ~31 | 12 | difference-of-rounded-rects, perspective |
| `aiks_dl_gradient_unittests.cc` | ~40 | 17 | dithering |
| `aiks_dl_clip_unittests.cc` | ~5 | 3 | difference clips |
| `aiks_dl_opacity_unittests.cc` | ~3 | 2 | subpass collapse |
| `aiks_dl_blend_unittests.cc` | ~79 | 36 | framebuffer fetch, wide gamut, subpass collapse |
| `aiks_dl_blur_unittests.cc` | ~59 | 24 | backdrop identity keys, mask blurs over a gradient |
| `aiks_dl_vertices_unittests.cc` | ~16 | 10 | runtime effects, mask filters on a mesh |
| `aiks_dl_atlas_unittests.cc` | ~15 | 5 | advanced blends per sprite, wide gamut, geometry-level cases |
| `aiks_dl_shadow_unittests.cc` | ~30 | 7 | a convex-shadow optimization this renderer does not have, and perspective |
| `aiks_dl_primitive_shape_unittests.cc` | ~2 | 0 | one is a playground harness, one is a hairline skew |
| `aiks_dl_text_unittests.cc` | — | 0 | text shaping and font parsing, out of scope |
| `aiks_dl_runtime_effect_unittests.cc` | — | 3 | bounded by having one fixture program rather than by the renderer |
| `aiks_dl_unittests.cc` | ~39 | 7 | mostly internal optimizations and picture round-trips |

The catalog holds one hundred and fifty-nine scenes of roughly four hundred,
and the proportion is less interesting than which ones: the arithmetic of drawing is largely covered, and
what is missing is either a capability this renderer does not have or a thing
the scene model cannot describe.

## What blocks the rest

Three different kinds of obstacle, worth separating because only one of them is
about the renderer.

**Capabilities this renderer lacks.** An effect's own textures — a caller's
program gets the material's uniform block and nothing else. Perspective
transforms, which the transform type is affine and two-dimensional by design.
Dithering. These are the rows of `docs/parity.md`, and the scenes that need
them arrive when the row does.

**Things the scene model could not describe.** This category is empty now, and
what was in it is worth keeping because of how it was closed rather than that
it was.

A scene has to be writable without a device — that is what lets one list serve
a window, a headless comparison and a board at the end of a cable — so a scene
cannot hold a texture. It does not need to: an item says it samples an image,
and the executor uploads a single fixture and binds it, for the scenes that ask
and allocating nothing for the ones that do not. A mesh and a sprite batch were
the other two, and they are their own kinds of node rather than kinds of item,
because an item is a shape with a fill and everything that follows from that —
a stroke, a clip built from its outline, a transform applied to its path — and
a mesh has none of them.

A caller's fragment program joined on the same terms and for the same reason. A
scene says it uses the fixture effect and the executor registers it, because a
program cannot be written down without a device any more than a texture can.
Registering the same payload twice gives the same name back, which is what lets
a caller with nowhere to keep an index — every caller that renders a list of
scenes — register before each one without building a pipeline per scene.

The lesson worth carrying is about the derivations. What a scene needs from a
device, what tolerance it earns, whether it samples the fixture: all three are
derived from what the scene contains rather than declared beside it, and all
three walked its *items*. A node kind that was not an item would have been
missed silently — a mesh using an advanced blend reported as a backend
regression rather than as a known gap. They ask the node now, exhaustively, so
the compiler will not let the next kind be added without a decision for each.

This category was larger when it was first written, and wrongly so: it claimed
a colour filter stated as a blend was among them. It was not. A filter is a
matrix by the time a scene carries one, and the constructor that builds one
from a blend mode had existed for days. Thirty-four blend scenes came across as
soon as somebody checked the claim instead of repeating it, which is the
argument for writing an inventory down rather than carrying it in one's head.

**A capability deliberately declined.** Round superellipses. Flutter's version
is not a closed form — each corner joins a superellipse arc to a circular one,
and the superellipse's degree comes from an eleven-entry lookup table
interpolated on the ratio of side to radius. Matching it means transcribing a
fitted table that nothing here could check, and drawing a different curve under
the same name would be worse than not drawing it. `docs/parity.md` has the
longer version.

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
