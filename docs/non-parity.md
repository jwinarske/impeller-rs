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
pixels. This renderer's mask blur sigma is already in device space, measured at
a seventeen-pixel tail under both a unit scale and a doubled one, so it arrives
at the same behavior by a shorter route. Copying the division would not add
parity; it would break it, by shrinking a shadow as the canvas grows.

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

## 7. Operations that are absent

These are listed in [`parity.md`](parity.md) with their reasoning and are
summarized here only so that this file is the one place to look.

- **`drawRSuperellipse` and `clipRSuperellipse`.** Flutter's rounded
  superellipse is not a closed form — each corner joins a superellipse arc to a
  circular one and the degree comes from an eleven-entry fitted table
  interpolated on the ratio of side to radius. Matching it means transcribing
  that table, and nothing here could check the transcription. Drawing a
  different curve under the same name would be worse than not drawing one.
  *Impact:* a caller who needs Flutter's squircle cannot get it.
- **Text shaping and font parsing.** Out of scope by design; `draw_glyphs` takes
  a positioned run and an atlas. *Impact:* a caller brings their own shaper.
- **`drawPicture` is tessellated rather than replayed.** `draw_recording`
  composes a finished recording into the current one by tessellating it again.
  *Impact:* the geometry is re-walked rather than the draws being replayed,
  which costs recording time on a repeated sub-picture.

## 8. One thing that looks like a difference and is not

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
