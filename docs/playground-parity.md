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
tests goes stale quietly. Six of them have now, and the two most recent
were written by whoever was checking the others -- which is the argument for
the suspicion rather than against it. Blurs under rotation and
clipping together were listed as blocked and were merely unwritten -- both
render, and both agree across the backends. Mipmapped cases were genuinely
blocked and are not any more. Runtime effects were listed as blocking the
vertices file and never did: a mesh without texture coordinates takes its
material from the paint's shader like any other geometry, so a caller's
program reached it through the ordinary path and nothing had to be built.

Difference-of-rounded-rects was the interesting one, because it was half
true. `draw_drrect` existed and had a test, so the renderer was not what
stopped it -- the scene format was, having no shape to say it with. A
capability the catalog cannot express is not covered by the catalog whatever
the API can do, and the cross-backend comparison is the thing being missed:
the only bug found so far in a backend's sampler bindings passed every test
on the other backend and was caught by a plate.

The two recent ones were of different kinds. "Wide gamut" went stale twice
over: the pipeline gained one, and while that was being written this document
said in the same breath that dithering was the only capability missing, which
was a contradiction no test here could see. "Mask blurs over a gradient" was
true when written and was then declared blocked on a decision that turned out
not to exist -- a mesh is refused elsewhere, a glyph run tints one color
whatever is done to it, and what was left was a shape with a fill that varies,
which is well defined and is now built.

"A hairline skew" survives as an obstacle but not as a description. A shear is
no difficulty: half a pixel of width under one draws exactly as it should, and
a test says so. What that scene wants is a stroke width of zero meaning the
thinnest line a device can draw, which is what `dart:ui` documents and is not
what it means here -- so it is a row of `docs/parity.md` rather than a gap in
the catalog, and upstream's own handling of it is unsettled enough that copying
it is not the obvious move.

| File | Scenes there | Here | Blocked on |
|---|---|---|---|
| `aiks_dl_basic_unittests.cc` | ~85 | 42 | superellipses, subpass optimizations |
| `aiks_dl_path_unittests.cc` | ~31 | 21 | nothing named; see below |
| `aiks_dl_gradient_unittests.cc` | ~40 | 31 | nothing; see below |
| `aiks_dl_clip_unittests.cc` | ~5 | 6 | nothing; this file is covered |
| `aiks_dl_opacity_unittests.cc` | ~3 | 2 | subpass collapse |
| `aiks_dl_blend_unittests.cc` | ~79 | 36 | framebuffer fetch, subpass collapse |
| `aiks_dl_blur_unittests.cc` | ~59 | 34 | backdrop identity keys, for two of them; see below |
| `aiks_dl_vertices_unittests.cc` | ~16 | 18 | mask filters on a mesh |
| `aiks_dl_atlas_unittests.cc` | ~15 | 9 | nothing named; see below |
| `aiks_dl_shadow_unittests.cc` | ~30 | 12 | a convex-shadow optimization this renderer does not have |
| `aiks_dl_primitive_shape_unittests.cc` | ~2 | 0 | one is a playground harness, one wants a stroke width of zero to mean a hairline |
| `aiks_dl_text_unittests.cc` | — | 5 | shaping and font parsing, which are out of scope; glyph rendering is not, and these use synthetic coverage |
| `aiks_dl_runtime_effect_unittests.cc` | — | 5 | bounded by having two fixture programs rather than by the renderer |
| `aiks_dl_unittests.cc` | ~39 | 10 | mostly internal optimizations; the picture cases are here now |

The catalog holds two hundred and thirty-one scenes of roughly four hundred,
and the proportion is less interesting than which ones: the arithmetic of drawing is largely covered, and
what is missing is either a capability this renderer does not have or a thing
the scene model cannot describe.

## What blocks the rest

Three different kinds of obstacle, worth separating because only one of them is
about the renderer.

**Capabilities this renderer lacks.** None, now that gradients are dithered.
It never was a row of `docs/parity.md` and should not be looked for as one: it
is not a `dart:ui` method but a quality behavior inside gradient rendering,
which is where the banding it exists to break up appears.

That sentence was written while two cells above it still said "wide gamut",
which made this document contradict itself for the length of one commit. Worth
recording rather than quietly fixing, because it is the failure mode this
column already has a warning about: prose no test reads goes stale, and the
person who wrote both halves is not exempt.

It went stale a second time, in the other direction and for much longer. The
gradient row went on saying "dithering" after the sentence above said there was
nothing left, so the two halves of the same claim sat nine lines apart
disagreeing, and the count under that row stayed where the blocker had left it.
Nobody had counted the chapter against the source -- this document said so
about wide gamut and the admission applied here too -- and counting it found
that nothing in the file was blocked at all. Eight scenes went in: the four
dithering plates the cell was named for, the incomplete-stops scene for the
three gradient kinds that were missing the one the linear kind already had, and
a gradient under a mask blur.

The blur row was wrong in a more interesting way, and checking it changed the
renderer rather than the document. It said "backdrop identity keys", which is
real -- upstream's `SaveLayer` takes a `backdrop_id` so several layers can share
one snapshot of what is behind them, and this renderer's `Layer` has no such
key. It governs two of the file's scenes. It was not what stopped the other
twenty-one, and writing the scenes is what found the thing that was: a mask
blur here took a solid color for three of its four styles.

The default style blurs coverage and fills through it, which works for any
fill. The three that combine a blurred mask with a sharp one -- solid, outer,
inner -- were built for a color only, and refused a gradient rather than
guessing at it. The refusal was right and the limit was not necessary: the
combination is the same rules whichever is being combined, so it now happens
once, over color where the fill is constant and over coverage where it varies,
and every style takes any fill. Eight scenes followed, five of them upstream's
stroked gradient oval under each style.

Fifteen of the gradient file's forty are still not mirrored, and none is
blocked either.
Four are upstream's fast-gradient scenes, which exist to check an optimization
that decides whether a two-stop gradient can be drawn as a full-screen quad --
an internal choice with no `dart:ui` surface, and one this renderer does not
make. The rest are tile modes crossed with many-colors, which is the one shape
of gap where writing the next scene is arithmetic rather than coverage: the
tile modes are each mirrored once already, and the many-colors ramp is mirrored
once already, and crossing them tests the crossing and nothing else.

The pipeline carries a wide gamut now. Colors state which primaries they are
against, a color outside the sRGB primaries' triangle keeps the components
below zero that say so, and it reaches a floating-point target through layers,
gradients and filters intact. **What is not built is presenting one.** The
swapchain still negotiates `SRGB_NONLINEAR` -- which is the presentation engine
being told the image holds encoded values, and it does -- and neither it nor the
scanout path asks for a wide-gamut color space, because the devices available
here are a software rasterizer and a virtual display controller: a Display P3
surface cannot be exercised, and this project does not ship what it cannot
check. So the two cells above no longer name wide
gamut, and nobody has counted against the source how many of those scenes
needed it rather than one of the other things listed.

Perspective transforms were the other, and were described here as a design
limit -- the transform type being affine and two-dimensional -- with the note
that the scenes needing it would arrive when the parity row changed. The row
changed, and three of them arrived: a curve under perspective, a receding
plane, and a rectangular clip under one. What that promise did not say, and
what is worth recording in its place, is that the count above moved by three
and not by the eleven the path chapter is still short. The blocked-on column
was per-file prose that no test reads, and it named one obstacle where there
may have been several; nobody has since counted, against the source, how many
of those scenes actually build a perspective matrix. So the path row now says
nothing is named rather than that nothing is left, which is the honest state of
it: the obstacle that was written down is gone, and what holds up the rest has
not been examined.

Dithering was not the small feature its one-word entry suggested, and the reason
is worth keeping even though the difficulty has since evaporated. Breaking up a
band means perturbing a color by a fraction of one quantization step before it
is quantized, and a step was not a fixed quantity while the pipeline carried
light: into a linear eight-bit surface it was a flat 1/255 of light, and into an
sRGB one the hardware encoded on write with a slope running from 12.92 at black
to roughly 0.44 at white -- so one offset was many steps in shadow and a
fraction of one in highlight. Measured rather than argued: dithering a dark
gradient into an sRGB target in *light* tracked the ideal about six times worse
than not dithering at all.

The pipeline carries encoded components now, and a target stores them without
transforming them, so a step is a flat 1/255 wherever it stands and the rate is
upstream's single `1.0 / 64.0`. The amplitude and the space it was applied in
are both gone from the paint block.

The lesson that outlives all of it is the one about the blocker. This was filed
as waiting on a decision about the color policy -- the conversion belonging to
the target format rather than to a shader -- and what was missed is that nothing
required the *shader* to be what knew the format. A backend fills two floats at
submission and the shader is handed numbers. The decision dissolved on being
looked at, which is the same lesson as the blocked column above.

Dithering the gradient's parameter rather than its color was the obvious way
around it and would not have worked: jittering where a band edge falls, rather
than what value it steps to, needs no knowledge of the target, but the jitter is
half a pixel wide while the band is widest exactly when the gradient changes
slowest. It is weakest where banding is worst.

The scope is upstream's, not a local judgment about where banding is worst.
Checked at tip of tree rather than from a checkout: `IPOrderedDither8x8` occurs
in six files there, its own definition plus the four SSBO gradient fills and the
two-stop fast path. The variants that sample a baked ramp do not call it. So a
gradient of four stops or fewer is dithered here and one tabulated into a ramp
is not, which costs the exact agreement between this renderer's two gradient
paths -- they now differ by what a dither reaches. That is a real price and
`docs/architecture.md` records it beside the claim it weakens.

One thing upstream does that is not followed: it compiles the dither out on
OpenGL ES, and the guard says why -- `mod` does not exist in GLES 2.0. This
backend's floor is GLES 3.0, so the limitation is not present, and copying the
workaround would make the two backends draw different pixels for no reason.

An effect's own textures used to head this list, on the grounds that a caller's
program got the material's uniform block and nothing else. That stopped being
true when the effect material grew texture slots, and the paragraph outlived the
limitation by some days -- which is the same failure the blocked column above is
warned about, in the prose that does the warning.

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
a color filter stated as a blend was among them. It was not. A filter is a
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
clear-color elision, peepholes — which assert about how a frame was rendered
rather than what it looks like, and which this renderer does not implement the
optimizations for.

## The scenes

Named for the test each mirrors, in the form `topic/CppTestName` reduced to
kebab case, so the original can be found by its name and a scene here with no
counterpart there would be visible as one. `catalog()` in
`crates/impeller-testkit/src/catalog.rs` is the list; the playground shows it
after the corpus, and `cargo test -p impeller-testkit --test catalog` renders
every one of them on both backends and compares.
