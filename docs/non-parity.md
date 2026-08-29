# Where this renderer knowingly differs from upstream Impeller

Technical parity with upstream Impeller is the criterion this project decides
against. This file is the list of places it does not have it, why, and what the
difference costs — so that a divergence is a decision somebody made and can
find, rather than something discovered later by whoever compares two pictures.

Two things this file is not. It is not the list of what is *unbuilt*: that is
[`parity.md`](parity.md) for the `dart:ui` surface and
[`playground-parity.md`](playground-parity.md) for the scenes. And it is not a
list of bugs — everything here is deliberate, and a difference that turns out
not to be deliberate belongs in a commit that removes it.

A fourth left it when the last of upstream's seven image filter kinds was
built. That entry had grown two paragraphs of scoping, and both turned out to
be arguing the job was larger than it was: the contract it worried about --
that a caller's program is handed its input as a texture with an identity
transform -- is one a filter pass here satisfies by construction, and the
storage question that looked like the real obstacle was answered by keeping the
program beside the layer rather than inside it. `Layer` is still `Copy` and
still a hundred and fifty-two bytes.

A third left it when a rounded rectangle here stopped having one radius. That
entry said the limit had never been a decision, only a generalization nobody had
written, and named where writing it would cost something -- the analytic route
is a signed distance to a shape with one circular radius, and eight numbers is a
different function rather than that one with more arguments. It was written the
way the entry said it should be: unequal corners tessellate, and a uniform
rounded rectangle still reaches the shader, including when a caller spells it
the general way.

Two entries left this file when the pipeline stopped working in light. Color was
linear here and encoded upstream, which was the deepest difference recorded and
the one most of the others followed from; and the dither's amplitude had to be
derived from the target because a step of the target was worth a different
amount of light at every brightness. Both are gone: the pipeline carries
sRGB-encoded components from the API boundary to the target write, as upstream's
does, and the dither is upstream's single `1.0 / 64.0`. What remains below is
what did not follow from that.

**Every upstream claim below was read at tip of tree**, in the `flutter/flutter`
monorepo under `engine/src/flutter/impeller`, not from a checkout. A parity
decision is worth exactly as much as the source it was read from, and a local
clone of unknown vintage can encode behavior upstream has since changed. Where
a claim names a symbol or a file, that is what to re-read when checking whether
this file has gone stale.

## 1. Four stops fit in the paint block; upstream carries 256

**What differs.** Past `MAX_STOPS` — four — the recorder tabulates a gradient
into a 256-texel ramp texture and the shader samples it. Upstream's
`kMaxUniformGradientStops` is 256, and its storage-buffer path is bounded only
by the buffer, so it walks stops in the shader for effectively every real
gradient and reaches a texture only past 256 stops or on a device without either
facility.

**Why.** The paint block is one uniform block per draw and every member of it is
a four-component vector; carrying 256 colors and 128 stop pairs would mean the
dedicated secondary blocks upstream uses, which is a different design for the
material rather than a larger one.

**Impact.** A five-stop gradient allocates and samples a texture here where
upstream would walk uniforms. The picture is meant to be the same, and is
tested to one level per channel: the ramp holds exactly what the four-stop walk
produces, so no quantization enters that the walk does not also have. The cost
is an upload and a sampler binding per gradient past four stops, on a path
upstream would not have taken.

Note the trap this sets, because it is easy to fall into and one commit here
already did. Upstream's *texture* path does not dither, and reading that across
to this renderer's ramp looks obviously right. It is backwards: upstream reaches
its texture past 256 stops and this renderer reaches its ramp past four, so
matching the mechanism would leave nearly every gradient here on the side
upstream nearly never uses. Both paths are dithered for that reason.

## 2. The gradient ramp is half-float; upstream's is eight-bit

**What differs.** `CreateGradientTexture` builds a
`PixelFormat::kR8G8B8A8UNormInt` texture. This renderer's ramp is
`Rgba16Float`. Both hold sRGB-encoded components; what differs is the precision
they hold them at.

**Why.** Range rather than precision. An eight-bit table cannot hold a component
outside the sRGB primaries at all, and a wide-gamut gradient has them — a
Display P3 red restated against sRGB is `1.093` in red and negative in the other
two. Upstream's table cannot carry that either, and reaches a table so rarely
that it has not had to.

**Impact.** Two kilobytes against one, per gradient past four stops. In exchange
a gradient stated in Display P3 survives being tabulated, and the two gradient
paths agree to a level rather than to twenty-four. It follows §1: upstream's
texture path is a fallback past 256 stops where this one is the ordinary path
past four, so a limitation upstream can live with is one this cannot.

## 3. Gradients are dithered on GLES

**What differs.** Upstream does not dither on OpenGL ES at all. Its fast path
guards the call with `#ifndef IMPELLER_TARGET_OPENGLES`; its storage-buffer path
is the only other one that dithers and needs storage buffers, which are ES 3.1;
and its uniform and texture paths never dither. Here both backends dither.

**Why.** The guard exists for a constraint this project does not have, and the
constraint is worth stating exactly rather than from the comment beside it. The
shader compiler defaults its GLES target to GLSL ES 1.00 —
`sl_options.version = ... : 100` in `impeller/compiler/compiler.cc` — which is
the OpenGL ES *2.0* shading language. It has no `uint`, no bitwise operators and
no `%`, and `IPOrderedDither8x8` is built from all three, so on that target the
function cannot compile at all. The comment beside the guard says "mod operator"
and understates it.

Two things follow. Upstream's GLES users lose dithering because the shader is
compiled once at that floor, not because anybody decided a gradient should band
there — a modern ES 3.0 device gets the undithered shader along with everything
else. And this backend's floor is GLES 3.0, with 2.0 permanently out of scope,
so `uint` and `%` are present and the same shader compiles and runs.

The second reason is load-bearing on its own. The cross-backend comparison holds
the two backends to `Tolerance::ROUNDING`, one unit per channel with no
outliers, while a dither reaches two — so importing the guard would fail the L3
lane on every gradient scene in the corpus, trading a real invariant for a
copied workaround to a limitation this renderer does not have.

**Impact.** A gradient drawn through this renderer's GLES backend is smoother
than the same gradient through upstream's. Nothing a caller can be harmed by,
but a direct comparison against upstream on a GLES device would differ by up to
two levels across the gradient, and would differ *only* there.

If upstream ever raises its GLES floor past 2.0, this entry should disappear
rather than be re-argued: the divergence is entirely downstream of that one
number.

## 4. Wide gamut is `Rgba16Float`, and is not presented

**What differs.** Upstream renders wide-gamut content into `BGRA10_XR`, a Metal
format that is extended-range ten-bit fixed point. Here the wide format is
`Rgba16Float`. And nothing here presents in a wide-gamut color space: the
swapchain format list, the color space it asks for, and the DRM scanout list are
all untouched.

**Why.** `BGRA10_XR` has no portable equivalent — the property that matters is
extended range rather than depth, and `Rgb10A2Unorm`, which does exist on both
backends, is unsigned and so cannot hold the negative component a Display P3 red
needs. On presentation: the devices available to this project are llvmpipe and
vkms, so a wide-gamut presentation path could not be checked, and would be code
whose correctness rested on having read a specification.

**Impact.** Eight bytes per pixel against upstream's eight, so no memory
difference. The pipeline carries the gamut and can be read back through it, but
a caller cannot get a wide-gamut image onto a display through this renderer, and
should not read the parity tables as saying otherwise.

## 5. A shadow's elevation is in device pixels

**What differs.** One thing, and it is not the blur's width, its color, or what
it does with an occluder — those were all on this list and none is now.
`DlDispatcherBase::drawShadow` takes a `dpr` and computes
`occluder_z = dpr * elevation`, so its elevation is in logical pixels. There is
no such parameter here and an elevation is in device pixels.

**Why.** An API difference rather than an omission. `dpr` is supplied by the
engine upstream and does not appear on `dart:ui`'s `Canvas.drawShadow` at all,
and this renderer has no notion of logical pixels to convert from — so an
elevation here means what it says.

**Impact.** A caller working in logical pixels has to scale the elevation
themselves, by the same factor they scale everything else.

Three things that were on this list and are not now, each removed by checking
rather than by deciding. The tonal color remap is ported. The occluder punch-out
is gone: upstream takes `transparent_occluder` and never reads it, and in the
arrangement the flag describes — an opaque caster drawn over its own shadow —
the punched and unpunched pictures were byte-identical while the punch cost a
layer, so every shadow was five passes where four will do. And
`drawShadow` divides its radius by `GetCurrentTransform().GetScale().y`, which
read as a divergence until both sides were measured: upstream's blur sigma is in
*local* space — `gaussian_blur_filter_contents.cc` multiplies it by
`ExtractScale(entity.GetTransform().Basis())` — so that division exists to
cancel the multiplication and leave the shadow's softness fixed in device
pixels.

That entry used to end here by saying this renderer's sigma was already in
device space and reached the same behavior without dividing, and that copying
the division would break parity rather than add it. Both halves were true of
the convention then in force and neither is now: the sigma is in the space the
drawing is in, as `dart:ui` states it and upstream honors it, so the
multiplication the division exists to cancel is here too and the division is
here with it. `docs/architecture.md` has the change and what caught the half of
it that was not designed. The behavior a caller sees is unchanged — a shadow's
softness is fixed in device pixels, measured at a five-pixel tail under a unit
scale and a doubled one — which is the point of both arrangements and the
reason this paragraph is a correction rather than a new entry.

Worth recording how the blur width was wrong, since the shape of the mistake is
more useful than the number. Elevation gives a kernel *radius*, and the blur
takes a *deviation*; upstream converts with `radius / sqrt(3) + 0.5`, and that
conversion was simply missing. Compounding it, the light ratio was read as
`800.0 / 600.0`. Upstream's dispatcher writes `constexpr Scalar kLightRadius =
800 / 600` with integer literals, so its value is one — while `DlCanvas` has a
*second* pair, `kShadowLightRadius` over `kShadowLightHeight`, which are floats
and do give one and a third, and which size the shadow's bounds rather than draw
it. Reading the wrong pair and skipping the conversion together made every
shadow here about twice as soft as the same elevation gives upstream.

## 6. A large blur is reduced by halving; upstream reduces in one step

**What differs.** Both shrink the image rather than spreading the taps once the
kernel outgrows its budget, and both clamp the deviation at five hundred. The
reduction is reached differently: upstream computes a downsample scalar and
resamples once through `texture_downsample.frag`, where this halves repeatedly
until the radius fits.

**Why.** A linear sample taken at the center of a two-by-two block averages
exactly those four texels, so halving *is* a box filter and a chain of halvings
needs no kernel of its own. Reducing by eight in one step with a single
bilinear tap would read four texels of every sixty-four and call the rest
absent, which is how a downsample turns a smooth image into a crawling one —
so a single-step reduction needs the dedicated shader upstream wrote for it,
and the chain does not.

**Impact.** Passes, and only past the threshold. Under a deviation of about
nineteen there is no reduction on either side and nothing differs. Above it this
spends one pass per halving where upstream spends one in total, so a very wide
blur costs two or three passes more — each on an image already a quarter or a
sixteenth of the size, which is why it was worth having the reduction at all.
The pictures agree: the reduction preserves light, checked at a deviation of
twenty-four by the energy test, which takes this path.

## 7. A blurred path that is not a rounded rectangle is blurred

**What differs.** Upstream has two ways of not running a blur pass. One is
built here and one is not.

- **A rounded rectangle**, all four corners sharing one circular radius:
  upstream's `AttemptDrawBlurredRRect` evaluates the blur in the fragment
  stage. **Built.** `Material::RoundedRectBlur` is Raph Levien's
  approximation, the method `SolidRRectBlurContents` evaluates, and a `Path`
  carries the shape that built it so a shadow reaches it too — `draw_shadow`
  takes a path, as `dart:ui` does, and upstream's `DlPath` answers the same
  question for the same reason.
- **Any other shape**: upstream's `DrawPath` sends a filled, solid-colored,
  positively-blurred path to `AttemptDrawBlurredPathSource`, which tessellates
  a **shadow mesh** whose vertices carry the falloff. **Not built.** Here it
  draws the shape into a layer and runs a separable Gaussian over it: one pass
  for the content and two for the blur.

**Impact, measured on a Raspberry Pi 5's V3D, release build.** The bench frame's
three shadows fall on rounded cards, so they now take the analytic route. The
frame costs **21.224 ms through Vulkan and 20.409 through GLES**, against
26.757 and 24.318 when they were blurred, and 18.928 and 17.676 with them left
out entirely. So three shadows cost 7.8 ms as passes and 2.3 ms as draws, and
the frame is five passes rather than fourteen.

What is left is the shape this does not cover. A shadow under anything that is
not a rounded rectangle — a rounded superellipse, a caller's outline, a glyph —
still costs three passes, and the mesh is what upstream answers that with.

The pictures agree either way, which is why [`parity.md`](parity.md) lists
`maskFilter` and `drawShadow` as built. This is a difference in what they cost.

## 8. A blurred rectangle is symmetric here; upstream's is not

**What differs.** One term, in the analytic blurred rounded rectangle. The
approximation shortens the longer axis by an amount that falls away as either
side grows past the deviation — a rectangle much longer than it is wide
otherwise blurs to something the axis-wise expression makes too eccentric.
Upstream writes that as

```c++
double delta = 1.25 * sigma * (eccentricV.x - eccentricV.y);
rSize += NegPos(delta);            // NegPos(v) = {min(v, 0), max(v, 0)}
```

which shortens x when x is the long axis and *lengthens* y when y is. This
renderer shortens whichever axis is longer: `{min(delta, 0), min(-delta, 0)}`.

**Why.** Upstream's own comment on that line reads "Pull in long end (make less
eccentric)", which is what it does in one orientation and the opposite of what
it does in the other. The consequence is visible: at a deviation of five, a
100×20 rectangle blurs as though it were 98.8 long and a 20×100 one as though
it were 101.2 — the same shape, turned, coming out two and a half texels
different. `a_blurred_rectangle_is_the_same_turned_either_way` fails by
twenty-four levels against upstream's form and passes against this one. The
sampled route passes either way, which is what placed the asymmetry in the
approximation rather than in the rasterizer.

Deviating rather than matching, because a blur whose width depends on which way
the rectangle is turned is a defect rather than a convention, and because
matching it would mean keeping a test that asserts the wrong thing. Worth
reporting upstream.

**Impact.** None on agreement with the sampled blur, which is the check that
matters for the approximation as a whole: the seven shapes in
`an_analytic_blurred_rectangle_agrees_with_the_blur_it_replaces` come to 13,
22, 17, 16, 18, 18 and 9 levels either way. The error was symmetric about the
sampled result — one orientation short, the other long — and is now the same
shortening in both.

## 9. Operations that are absent

These are listed in [`parity.md`](parity.md) with their reasoning and are
summarized here only so that this file is the one place to look.

- **Text shaping and font parsing.** Out of scope by design; `draw_glyphs` takes
  a positioned run and an atlas. *Impact:* a caller brings their own shaper.
- **`drawPicture` is tessellated rather than replayed.** `draw_recording`
  composes a finished recording into the current one by tessellating it again.
  *Impact:* the geometry is re-walked rather than the draws being replayed,
  which costs recording time on a repeated sub-picture.

## 10. An upstream artifact carried on purpose

**The squircle's outline snaps at twelve corner radii, and it does here too.**

`draw_rsuperellipse` approximates each superellipse arc with two conics, and
the conic weights come from a fitted table interpolated on the curve's degree.
Upstream's interpolation multiplies `sqrt(n)` into only the right-hand term, so
the weight climbs across each interval and drops back at the next whole degree
-- a sawtooth with a forty percent step, twelve times over the table's range.

It shows. Sweeping the ratio of side to corner radius and measuring the drawn
outline against the analytic curve, a ratio of 2.700 lands within 0.005 of the
true shape and 2.705 lands 0.042 away. Two tenths of a percent of corner
radius, a ninefold change in how faithful the outline is, at a place where the
shape itself is perfectly continuous. A control animating its corner radius
crosses several of these.

The obvious repair -- applying the factor to the whole interpolation, which is
what upstream's own comment describes -- was implemented here and measured, and
it is worse everywhere: 0.056 at its worst against 0.046, and two to five times
the error past a ratio of five. The table was fitted against the formula as
written, so correcting the formula without refitting the table moves the shape
further from the curve it is approximating rather than closer.

So this is carried rather than fixed. Smoothing it would put this renderer's
squircle where Flutter's is not, which is the substitution refused everywhere
else here; refitting the table would be inventing a shape rather than matching
one. *Impact:* none against upstream, which is the point -- the outline is
wrong in exactly the way Flutter's is. It is written down because the next
person to measure this shape will find the jump and reasonably think it is a
local mistake.

## 11. One thing that looks like a difference and is not

Worth stating because a reviewer raised it as a hole. **The advanced blend modes
are defined on `[0, 1]` here and clip in `set_lum`,** which looks like an
eight-bit assumption surviving into a wide-gamut pipeline. It is not: that clip
is the W3C compositing specification's `ClipColor`, part of the *definition* of
the non-separable modes, and upstream implements the same specification.
Matching it is parity. Extending those modes past the unit range would be
inventing behavior upstream does not have.

**Impact.** None, which is the reason for the entry. It is here so that the
next reader who notices the clip finds the answer rather than filing it, and so
that anyone tempted to "fix" it sees that doing so would *create* a divergence
rather than remove one.

## 12. A layer's matrix cannot bring content in from off the target

**What differs.** `Layer::with_matrix` is `dart:ui`'s matrix image filter on a
save layer, and it resamples what the layer captured. What the layer captured
is bounded by its target, and a layer's target never exceeds its parent's -- so
a shape drawn outside the parent is gone before the matrix runs, and a
translation that would have brought it into view brings in nothing.

Upstream sizes the layer through the filter: it computes the coverage the
filtered result will occupy and allocates for that, so a circle drawn three
hundred points to the left of the frame inside a layer translated three hundred
to the right renders. Its
`MatrixImageFilterDoesntCullWhenTranslatedFromOffscreen` is that case by name.
Measured here, the same picture draws nothing at all -- not a clipped circle, no
pixels.

Fixing it means sizing a matrix layer's target to the pre-image of the visible
region rather than to the parent, which is a memory decision as much as an
arithmetic one: the inverse of a minifying matrix is a magnifying one, and
`docs/architecture.md` records that layer allocation is already where this
project has run a machine out of texture memory. So it is written down rather
than done, and pinned by
`a_layer_matrix_does_not_recover_what_fell_outside_the_layer` so that building
it is a test that changes rather than a behavior nobody had noticed.

**Impact.** A caller who moves a layer *within* the frame sees no difference,
which is nearly every use: a matrix filter is usually a scale or a small
translation applied to something already drawn where it can be seen. A caller
who draws deliberately off-target and translates it in gets nothing, where
upstream gets the picture. There is no partial failure between the two -- the
content is either inside the target or absent -- so the case is visible the
first time it is tried rather than subtly wrong.

## 13. A translucent stroke laid down by the tessellator covers a pixel twice

**What differs.** A stroke is tessellated as a run of quads with a join between
each pair and a cap on each end, and where those quads land on the same pixel
the outline covers it more than once. At full opacity that is invisible. At
half it is not: each cover blends over the last, so the pixel comes out darker
than a stroke of that alpha should ever be.

Upstream draws exactly this picture to say it does not happen. Its
`CanRenderWideStrokedRectWithoutOverlap` and its `...RectPath...` twin are the
same six outlines, translucent blue, three joins where the stroke leaves a gap
down the middle and three where it is wider than the shape it outlines -- one
saying the rectangle directly and one handing over its path, so that neither
route may double.

Measured here on the second row, where the stroke is twice the width of the
rectangle. Through `draw_rect` with a round join, which this renderer answers
analytically, every pixel of the middle carries one cover and reads 140 in
blue. Through the tessellator -- which is what the other two joins get, a
stroked rectangle with a square corner having no analytic form here, and what
all three get when the scene hands over a path -- the same pixels read 226 and
251, which is three covers and six.

So the analytic route is right and the tessellated one is not, and the two
plates are in the catalog side by side to keep the difference visible. The
analytic half is pinned by
`a_wide_stroke_through_its_own_call_covers_each_pixel_once`, which fails if that
route ever starts doubling too.

Fixing it is not a change to the stroker. The quads have to overlap -- that is
how a join covers the wedge between two segments -- so what has to change is
that the whole outline is resolved to coverage before the paint's alpha is
applied, rather than each quad blending as it arrives. That is a stencil pass or
an offscreen per stroke, and it is a cost every stroke would pay for a case only
a translucent self-overlapping one has. Upstream's own route to the picture
suggests the cheaper answer first: it turns a stroked rectangle into an
analytic rounded rectangle where it can, which is the same move this renderer
already makes for the round join and declines for the other two.

**Impact.** Confined to a translucent stroke wide enough to reach across the
shape it outlines, or one whose path doubles back on itself inside a stroke
width. An opaque stroke of any width is unaffected, and so is a translucent one
narrow relative to its geometry, which is nearly every stroke drawn. Where it
does show, it shows as a darker patch at the joins rather than as anything
structural, and it is the same on both backends.

## 14. Only an image can be read at a mesh's texture coordinates

**What differs.** `draw_vertices` takes a per-vertex texture coordinate, and
here that coordinate is only ever read by an image: a mesh carrying coordinates
with anything else on the paint is refused with "a mesh with texture
coordinates needs an image paint to read". Upstream reads whatever the paint's
color source is at those coordinates, image or not, and keeps two scenes on it
-- `DrawVerticesLinearGradientWithTextureCoordinates`, which runs a linear ramp
across a triangle in a direction the triangle's own shape does not suggest, and
`DrawVerticesTextureCoordinatesWithFragmentShader`, which does the same with a
runtime effect.

The reason is in the shader rather than in the API. A gradient's coordinate
comes from `to_gradient_space(in.clip)` -- the fragment's position carried back
through the inverse of what placed the geometry -- while an image's comes from
`in.uv`, the interpolated attribute. The vertex already carries the coordinate
in both cases; nothing reads it except the image branch. So the change is a
material that says which of the two a shader takes its coordinate from, and a
branch in the shader for every gradient draw to honor it.

That is worth stating precisely because the refusal, as it stood, said nothing.
It was a bare `return Err` with no comment beside it, in a file where the
refusals around it each carry a paragraph on why substituting something would
be worse. This one is not that kind of refusal: there is no argument that
reading a gradient at a mesh's coordinates is the wrong picture, and upstream
draws it. It is unbuilt, and it was recorded as though it were decided.

**Impact.** A caller who gives a mesh texture coordinates and a paint that is
not an image gets an error rather than a picture, which is visible the first
time it is tried rather than subtly wrong. A mesh with no texture coordinates
is unaffected and takes the paint exactly as a path does, which is the common
case: coordinates exist to place an image, and a caller who wanted a gradient
across a mesh usually states it in the mesh's own space and needs no
coordinates at all.

## 15. A blur runs along the device's axes, so a rotation does not turn it

**What differs.** `ImageFilter::Blur` now carries a deviation per axis, as
`dart:ui`'s `ImageFilter.blur` does. The two passes it runs are along the
target's own axes, which is right while the two agree with the caller's and is
wrong as soon as they do not: a layer turned forty-five degrees with a blur
along x alone spreads along the *screen's* x, not along the axis the caller
stated it in.

Measured. A thirty-two pixel square blurred with a deviation of ten in x and
none in y covers thirty-nine by fifteen upright. Turned a quarter of a right
angle it covers forty-five by twenty-one -- which is the same horizontal smear
applied to a diamond, not a smear that turned with it. Upstream's
`GaussianBlurRotatedNonUniform` is that case by name, and it is the one scene of
its file that is not mirrored here for this reason.

An isotropic blur is unaffected, which is why nothing had noticed: a rotation
takes a circular kernel to a circular kernel, and every blur in this repository
was circular until the second deviation existed.

**What upstream does**, read at tip of tree in
`impeller/entity/contents/filters/gaussian_blur_filter_contents.cc`, and it is
not what a first guess suggests. It does not turn the blur. It removes the
rotation from the space the blur happens in and puts it back afterwards:

    // Source space here is scaled by the entity's transform. [...] You can
    // think of this as "scaled source space" or "un-rotated local space". The
    // entity's rotation is applied to the result of the blur as part of the
    // result's transform.

`ExtractScale` takes the *lengths* of the transformed basis vectors, so a
rotation contributes nothing to it; the input is then re-rendered under
`MakeTranslation(offset) * MakeScale(scale)` alone, blurred along that space's
own axes with a sigma per axis, and the finished image is drawn back under the
full transform, rotation included. A `FML_DCHECK` on the snapshot's transform
being translation-and-scale only holds the invariant in place.

So the blur is always axis-aligned, and the rotation is a resample of the
blurred result. That is a quality decision as much as an implementation one --
the content is rasterized un-rotated and then turned -- and it is why upstream
needs no separable-blur-along-a-rotated-basis and no fallback for a shear.

An earlier version of this entry said the fix was for the layer to carry a
basis where it carries a scale. That was a guess and it was wrong in kind. What
this renderer would need is upstream's arrangement: a layer opened under a
rotation would have to take its target in the un-rotated space -- scale and
translation only, which is already what `Layer::scaled_by` extracts -- draw its
contents there, and carry the rotation to the composite instead. The pass's
`step` staying axis-aligned is then correct rather than a limitation.

**Impact.** Confined to a blur whose two deviations differ *and* which is drawn
under a rotation. A blur stated with one deviation is unaffected at any
transform. A blur with two under a translation, a scale or no transform at all
is correct, which is the case a caller reaching for `sigmaX` and `sigmaY`
usually has -- a horizontal smear on upright content. Where it does bite it is
visible rather than subtle: the smear points the wrong way.

## 16. A mask blur under a mode that ignores coverage erases its whole bounds

**What differs.** A mask blur that cannot be evaluated in the fragment stage is
drawn as a layer: the shape goes into a target, the target is blurred, and the
layer is composited onto the frame with the caller's blend. That composite
covers the layer's *bounds*, and a mode that writes where its source is
transparent writes across all of them -- so the shape becomes its bounding
rectangle.

Seven of `dart:ui`'s modes ignore coverage in that sense: the ones whose
destination factor is neither `One` nor `OneMinusSrcAlpha` -- `Clear`, `Src`,
`SrcIn`, `SrcOut`, `DstIn`, `DstATop` and `Modulate`. Every other mode,
`SrcOver` and all the advanced ones included, is unaffected at any deviation.

**`Clear` is fixed, and the other six are not.** That split is upstream's and
not an arbitrary stopping point. `Clear` is the one mode that can be admitted to
the *evaluated* blur anyway, because on a coverage it is not what its factors
say: clearing by an amount `c` is `dst * (1 - c)`, which is `DstOut` against a
white source -- and white is exact rather than approximate, since `Clear`
discards the source color by definition and cannot care which one it had. So the
guard that refuses a coverage-ignoring mode admits `Clear`, substituting white
and `DstOut`, and a blurred circle drawn to clear now erases by its falloff:
alpha climbs monotonically out of the hole, and a corner of what the bounds
would have been is untouched.

Upstream does exactly this and no more.
`SolidRRectLikeBlurContents::Render` checks for `BlendMode::kClear`, forces the
color to white, and sets a flag that turns the pipeline's blend into a reverse
subtraction -- destination factor `One`, source factor `DestinationColor`, so
`dst - src * dst`. Same arithmetic; upstream reaches it by subtraction because
its fragment writes coverage directly, and this reaches it by naming the mode
that already means it. The other six have no such reading, upstream does not
generalize the case, and neither does this.

**What is left.** Two residues, and both are the layer route rather than the
evaluated one. The other six modes over any mask blur. And `Clear` over a shape
that is not rounded-rectangle-like -- a polygon, a curve -- which has no
evaluated blur to be admitted to and falls through to the layer as before.
Fixing either means treating a layer's alpha as coverage rather than as an
image, `mix(dst, M(src, dst), src_alpha)` per mode, which is a table nobody
upstream has derived either.

**Impact.** Confined to a mask blur combined with one of those six modes, or to
`Clear` on a shape with no analytic form. `SrcOver` is what nearly every blurred
draw uses. Where it does bite it is loud rather than subtle: a rectangle appears
where a soft shape was asked for.
