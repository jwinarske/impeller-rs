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
the catalog.

This used to add that upstream's own handling was unsettled enough that copying
it was not the obvious move. Read at tip of tree, that is not so, at least where
upstream has decided: `line_geometry.cc` pixel-aligns a line whose width is
zero, and `canvas.cc` says of the same case that it "draws a hairline that is
always 1 pixel regardless of the transform". What is not settled is the rest --
the general stroke path multiplies the width by a half with no special case, so
a stroked circle at zero appears to produce nothing there either, which is what
this scene draws. So the deviation stands for now and its description does not:
it is one this project has decided against upstream's decided half, not a gap
nobody upstream has filled.

All fourteen of the rows below have now been read against the file they name, at
tip of tree, and the results are set out after the table. Three named their obstacle correctly. Text, where nearly all forty of that
file's scenes shape a real font and mirroring them would mean shipping a
shaper. And: shadows, where it governs twenty-six of that file's thirty scenes and
means what it says, and paths, where nothing blocks the nine it is short. The
path row's count was still one too high, for the reason the atlas row's was four
too high -- a test in the file that draws no picture -- so of the two, one was
right outright and one was right about the only thing the column claims to be
about.

That makes three right and eleven wrong, in eight distinguishable ways, and the
ways matter more than the count.

An obstacle that had been removed and the row not updated (gradients). One
stated far more broadly than it held, two scenes rather than a chapter (blurs).
One resting on a precedent that a later change took away (meshes). One read off
the shape of a scene rather than off anything standing in the way (nested
opacity). One naming an upstream uncertainty that upstream has since resolved
(primitive shapes). One naming real obstacles but for three of twenty-one
tests, with a dozen unwritten (blends). One understating badly and missing the
only real obstacle in its chapter, which was not recorded anywhere -- that a
rounded rectangle here has one radius where `dart:ui` has eight (basics). One whose "see below" pointed at nothing, and whose own count turned out to be
the thing that was wrong (atlases). One that said its file was covered when
it was not, by a single scene that needed a capability the preferred device here
lacks -- which is how it came to be overlooked (clips). One that said its
picture cases were all present when two thirds of them were not (miscellany).
And one that said its chapter was bounded by test fixtures rather than by the
renderer, when the renderer is exactly what bounds it (runtime effects).

Three of the eight turned into renderer changes rather than document ones.

All fourteen have now been checked. Treat them as unverified, which is what they are.
On the evidence above, the likeliest thing wrong with them is not that they
name the wrong obstacle but that they name a real one governing far less than
the gap it is offered to explain.

One thing the checking did *not* find, and it is worth saying because the
opposite was nearly recorded: the "Scenes there" counts are in scenes and not
in tests. Two of them look badly stale against a count of `TEST_P` -- the blend
file holds twenty-one and the row says about eighty -- and they are not, because
`IMPELLER_FOR_EACH_BLEND_MODE` expands one test over every blend mode. The
column is right and the obvious way to check it is wrong.

| File | Scenes there | Here | Blocked on |
|---|---|---|---|
| `aiks_dl_basic_unittests.cc` | ~85 | 42 | superellipses, subpass optimizations; see below |
| `aiks_dl_path_unittests.cc` | ~30 | 21 | nothing; see below |
| `aiks_dl_gradient_unittests.cc` | ~40 | 31 | nothing; see below |
| `aiks_dl_clip_unittests.cc` | ~5 | 7 | nothing; this file is covered |
| `aiks_dl_opacity_unittests.cc` | ~3 | 3 | nothing; this file is covered |
| `aiks_dl_blend_unittests.cc` | ~79 | 39 | framebuffer fetch and subpass collapse, for three of them; see below |
| `aiks_dl_blur_unittests.cc` | ~59 | 34 | backdrop identity keys, for two of them; see below |
| `aiks_dl_vertices_unittests.cc` | ~16 | 19 | nothing; see below |
| `aiks_dl_atlas_unittests.cc` | ~11 | 9 | nothing; see below |
| `aiks_dl_shadow_unittests.cc` | ~30 | 13 | a convex-shadow optimization this renderer does not have; see below |
| `aiks_dl_primitive_shape_unittests.cc` | ~2 | 0 | one is a playground harness, one wants a stroke width of zero to mean a hairline |
| `aiks_dl_text_unittests.cc` | — | 7 | shaping and font parsing, which are out of scope; glyph rendering is not, and these use synthetic coverage |
| `aiks_dl_runtime_effect_unittests.cc` | — | 12 | nothing; see below |
| `aiks_dl_unittests.cc` | ~36 | 15 | subpass collapse, for five of them; see below |

The catalog holds two hundred and fifty-two scenes of roughly four hundred,
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

The last two rows are the two whose obstacle is a boundary this project drew,
and they came out opposite ways.

The text row is right. Nearly all forty of that file's scenes shape a real font
-- emoji, italics, subpixel alignment, a shadow cache keyed on glyph identity --
and a font file decides those pictures, so mirroring them would mean shipping a
shaper. Two that are about glyph *rendering* rather than about text were
mirrorable with synthetic coverage and are here now: a run inside a save layer,
where the atlas is resampled once more than it otherwise would be, and a run
that leaves the frame on both sides, which is where a coordinate computed from a
quad's own corners rather than from its visible part goes wrong. One that looked
mirrorable is not -- `TextForegroundShaderWithTransform` puts a gradient over a
run, and a run here takes a solid color, which `docs/parity.md` says in the
`maskFilter` row and `docs/non-parity.md` explains.

The runtime-effect row is wrong, and wrong in the way that matters most: it said
the chapter was "bounded by having two fixture programs rather than by the
renderer", and it is bounded by the renderer. Upstream's `DlImageFilter` offered
seven kinds where `ImageFilter` offered five. Both missing kinds have since been
built, so the row's obstacle is gone. Three of the twelve are mirrored now: a
program used as an image filter, and the pair that composes one with a blur
both ways round. The pair is the point rather than either half -- composing is
the one thing a program used as a filter can do that a program used as a paint
cannot, and the two orders come out thirty-six per cent apart, which is what
says the order is carried rather than collapsed.

The plates fill with a gradient where a flat color would have been easier, and
the reason is a mutation that passed. Where a texture binding names nothing the
backend binds a placeholder, so a program handed no input still samples
something -- and against a flat fill, tinting the placeholder is the same
picture as tinting the layer. A gradient is not reproducible that way.

Four more followed once a backdrop could be filtered by something other than a
blur, which is the other half of what those scenes need: a group whose backdrop
is a program, composed with a blur, bounded and unbounded. They are drawn at one
sample, for the reason the blur chapter's backdrop plates are -- a backdrop cuts
the pass to read what it was writing, and a multisampled pass cannot be resumed.

Their ground is a gradient with hard-edged bars over it, and the bars were added
after measuring. On a smooth ramp a blur does so little that the two sigmas came
out four levels apart and the inner half of the composition was barely tested;
with edges to work on they are a hundred and thirty-three apart over
ninety-seven per cent of the frame.

Writing them found a bug that a catalog cannot show. The fixture programs were
registered by a derivation that walked a scene for anything naming one, and a
group whose *backdrop* is a program has nothing inside it that does -- so they
went unregistered, and the plates drew correctly anyway because an earlier scene
had registered them and registration is idempotent. Alone they failed every
time. The derivation had been wrong four times by then and is deleted rather
than corrected: registering stores a program's SPIR-V against an index and
pipelines are built lazily from it, so the gate was buying three clones of a
byte vector per context at the price of having to be right. There is a test now
that renders one scene per context for each way a scene can depend on something
the executor arranges.

Seven of the twelve rested on the runtime effect
-- every `ComposePaintRuntime` and `ComposeBackdropRuntime` variant,
`CanRenderRuntimeEffectFilter`, `RuntimeEffectImageFilterRotated` and
`ClippedBackdropFilterWithShader`. A fragment program is a paint here, so it can
fill a shape and cannot filter what a layer already drew. That is now
`docs/non-parity.md` §7, and the `imageFilter` row of `docs/parity.md` names
both absences rather than only listing what it has.

The miscellany row said "mostly internal optimizations; the picture cases are
here now", and the second half was the false one. Thirty-eight tests, two of
which are unit tests on a texture -- `EXPECT_EQ(texture, nullptr)` and
`EXPECT_FALSE(texture->NeedsMipmapGeneration())` -- so thirty-six pictures
against twelve here. Five of the twenty-four missing are subpass collapse and
are named for it; the rest are pictures, and the largest family among them is
eight translucent save layers, each with a different filter on it.

Five of those eight are here now, and the family divides in two once a group
can be filtered as a whole. Two recolor the group on its way out, two run a
filter over the finished group, and one does both -- and the two halves are not
two spellings of one thing: the same blend against a constant, put one way and
then the other, differs across the whole frame.

The family is the point rather than any one of
them: a translucent group has two things that have to happen in the right
order, the alpha it composites with and whatever recolors it on the way out, and
only a filter that is not a plain scale can tell the orders apart. So neither of
the two is one. Destination-over against a constant reads the group's own alpha,
so what the filter contributes is strongest where the group is thinnest; and
upstream's alpha-doubling matrix is chosen to fight the layer's alpha rather
than to look like anything.

Checked by rendering all three of the family that now exist and comparing them,
because a filter that was accepted and dropped would leave the plates identical
and passing. They differ across the whole frame and across a third of it
respectively, which is the shape each filter should have.

A count that was nearly wrong the other way, worth recording beside the two
that were: five of that file's tests read like resource tests from their names
-- mipmap generation, releasing a texture on teardown, setting contents with a
region, two about depth values -- and all five open a playground and are
scenes. Only the two named above do not.

The clip row said "nothing; this file is covered" and the file was not covered.
Six scenes against five tests reads as a surplus, and two of the six are this
chapter's own, so four of upstream's five were mirrored and the fifth was not.
`FramebufferBlendsRespectClips` is now here and the row's claim is now true.

It is worth a plate rather than being folded into the blend chapter, because
what it is about is the clip and not the mode. A separable blend is applied by
the hardware as it writes, so the clip has already decided which pixels are
written and there is nothing further to respect. An advanced mode reads its
destination -- through a framebuffer fetch, or through a copy of the target --
and a reader that ignores the clip blends happily into pixels no draw should
have touched. The plate multiplies a red square across a region much larger
than the circle it is clipped to, and its corners are the answer: pure white on
a software device, which is the ground exactly as it was.

That plate is also the first thing to be drawn by the device fix two commits
ago rather than in spite of it. `Multiply` is an advanced mode and the
preferred device here has no extension for it, so before that change this
scene would have been skipped by the only test that draws every plate, and
skipped silently.

The path row is the second whose count was the thing that was wrong, and by one
rather than by four. `ArcWithZeroSweepAndBlur` builds a display list and stops,
with a comment saying that an empty picture has to be creatable without
crashing; it opens no playground and is not a scene. Thirty pictures, twenty-one
here, nine short -- and nothing blocking any of the nine, which are strokes,
lines drawn four different ways, multi-contour paths and a fat stroked arc.

That crash test is worth having and is not worth a plate, so it is a test. An
arc of zero sweep is a single point, a sweep gradient divides by an angle, and a
mask blur opens layers around whatever coverage the point produced -- three
things with degenerate cases meeting on one draw. It records here without
complaint and reaches a device, and the frame comes back untouched, which is
what says the point produced no coverage rather than coverage nobody looked at.

The atlas row said "nothing named; see below" and there was nothing below --
the only row whose pointer went nowhere, which is its own kind of stale.
Checking it moved the other column instead.

That file holds fifteen tests and eleven pictures. Four of them --
`DlAtlasGeometryNoBlendRenamed`, `DlAtlasGeometryBlend`,
`DlAtlasGeometryColorButNoBlend` and `DlAtlasGeometrySkip` -- never open a
playground at all: they build an atlas geometry and assert on its flags and its
vertex buffer, `EXPECT_TRUE(geom.ShouldSkip())` and the like. They are unit
tests that happen to live in the playground file, and counting them as scenes
made the chapter look a third emptier than it is. So the count beside it is
eleven, and the nine here are nine of eleven.

Of the two that are neither mirrored nor unit tests, plus the four advanced ones
that are: three need advanced blending, which nothing on the machines this was
written on has, so they would be reported as gaps rather than compared; one is
`Plus` into a wide-gamut target, which §4 of `docs/non-parity.md` covers; and
two compare upstream's conversion of `drawImageRect` into a `drawAtlas` against
the unconverted path. That conversion is an optimization with no counterpart
here, and the picture the pair would contribute -- an image under a color filter
-- is already `basic/can-render-inverted-image-with-color-filter`.

This is the one place the counting caveat above cuts the other way. The "Scenes
there" column is in scenes and not tests, which is why the blend row's eighty
is right against twenty-one tests; here the same distinction makes fifteen
wrong against eleven pictures. Both directions have now been checked once each.

The basic row named superellipses and subpass optimizations, and understated
what it was covering. Superellipses account for seven of that file's
eighty-five, and the file is short by forty-one -- so the two names between
them explain a fraction of the gap, and the rest of the missing scenes are
`CanDrawPaint`, `CanPerformSkew`, `CanRenderSimpleClips`, a dozen `StrokedArcs`
variants and other ordinary pictures that are unwritten rather than blocked.

Checking it did find one obstacle nobody had written down, and it is a real
one. A rounded rectangle here has a single circular radius; `dart:ui`'s `RRect`
and upstream's `RoundRect` both carry four corners with independent x and y
radii, eight numbers against one. Five of the basic chapter's scenes build a
rounded rectangle this cannot describe. That was not in this column and, worse,
was not in `docs/parity.md` either, whose `drawRRect` row said "yes" with
nothing beside it.

It was recorded as a non-parity entry and then, a commit later, built instead --
which is what the entry had said should happen to it, having named the limit as
a generalization nobody had written rather than a decision anybody took. So the
five scenes are no longer blocked, and this row is short by superellipses,
subpass optimizations and forty ordinary pictures nobody has written.

The blend row named two obstacles and both are real, for three of that file's
twenty-one tests: two need framebuffer fetch by name, and one is the subpass
collapse optimization itself. Twelve of the rest were unwritten rather than
blocked, and three of those are here now -- `drawPaint` twice under a
non-separable mode, an advanced blend clipped so its destination runs out, and
an empty group whose color filter floods what its bounds admit. That last is
the interesting one: nothing is drawn inside the group, so it is transparent,
and a filter that ignores its input turns transparent into opaque red. It comes
out solid red on both devices here, which is what upstream's comment says it
should be.

Checking that row turned up something larger than the row. Two of the three new
scenes need advanced blending, which the preferred device on the machine this
was written on does not have -- and `every_catalog_scene_draws_something` asked
only the preferred device and skipped what it could not render, without saying
so. Nineteen plates were never drawn at all, and the test reported a pass. The
software rasterizer beside it has the extension and renders every one of them.

So that test now asks every Vulkan device on the machine, in the order the
conformance suite already asks them, and says how many of the catalog it drew.
It draws all two hundred and thirty-seven. The cross-backend comparison beside
it was already honest -- it reports its nineteen gaps by name -- and those stay
gaps, because the GLES driver here lacks the extension too, so there is no pair
to compare them on. Reported, not hidden, which was the whole difference.

The shadow row is the first of these to survive being checked, and is worth
recording for that rather than in spite of it. Twenty-six of that file's thirty
scenes are named `DrawShadowCanOptimize` or `DrawShadowDoesNotOptimize`
something, and what they are for is deciding which paths upstream's
convex-shadow optimization applies to. That optimization is not here. The
scenes would still *draw* -- an optimization that changed the picture would be
a bug -- so they are not blocked in the way the column's other entries claimed
to be; they are scenes whose whole subject is a thing this renderer does not
do, and mirroring them would produce pictures that check nothing. That is a
better reason not to write them than being unable to, and the row means it.

One of the four that are not about the optimization was missing and is now
here. `CanDrawPerspectiveConvexShadow` is filed with the others and is not one
of them: it draws a mask blur under a three-dimensional rotation and a
perspective matrix, where its two siblings rotate and scale. Its siblings were
already translated as a shadow under a transform, and perspective is a field of
that transform, so the third followed once perspective existed. It did not
exist when the row was written, which is the other way one of these goes stale.

The opacity row said "subpass collapse", and its one missing scene needed
nothing of the sort. Subpass collapse is upstream's optimization for a save
layer whose contents let it be folded into its parent, which is what that
scene's nested layers are shaped like -- so the name was read off the scene
rather than off any obstacle. It decides how many passes a picture costs, not
what the picture is. The scene draws, the file is covered, and what the scene
is actually about turned out to be unchecked anywhere: that a group's alpha
applies to the finished group rather than to each draw in it, and that nesting
two of them multiplies. There is a test for that now.

Its first draft could not have failed. It compared a nested pair against a
single layer worth their product, and every such comparison survives
compositing each layer at the square root of its alpha -- the root of
forty-nine hundredths is seven tenths, and seven tenths squared is back where
it started. So it now also asserts what the alpha is worth against a value
derived rather than measured, and both mutations fail it.

The vertices row said "mask filters on a mesh", and that was true: a mask blur
over one was refused outright. The reason given was that a mesh carries a color
per vertex and so varies by construction, and it does not have to -- built from
positions alone it is filled by the paint, exactly as a path covering the same
area is. Upstream's own scene for this passes no vertex colors at all.

What the refusal was protecting is real and is narrower than it was written.
A mask blur fills through blurred coverage, so the fill must have a value
everywhere that coverage reaches, halo included. A paint has one, being a
function of position. Per-vertex colors are defined on the triangles and nowhere
else, so the halo has nothing to take its color from, and that case is still
refused rather than extrapolated -- the message now says so instead of saying
meshes. A mesh the paint fills now blurs, and is required to come out identical
to the same square drawn as a path.

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
and not by the nine the path chapter is still short -- eleven when that was
written, and the number has been checked since rather than carried forward.
The blocked-on column
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
