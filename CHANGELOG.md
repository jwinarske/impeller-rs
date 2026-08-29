# Changelog

This file starts at the point there was a version on a registry to anchor it
to. Everything before that is in the commit history, which is written to be
read: a commit here explains why a change is shaped the way it is rather than
restating what moved.

Dates are the day a version reached crates.io.

## Unreleased

Everything. No version with an API has been published, so this section holds
the whole project rather than a delta, and `docs/parity.md` and
`docs/playground-parity.md` describe its state far better than a list could —
both are checked by tests, so neither can drift from the code without failing
the build.

This section used to carry a list of what stood between here and a first release
with an API. It had one entry, `drawRSuperellipse` and `clipRSuperellipse`, and
it stayed on the list for a while after both were written -- which is the
duplication the paragraph above rules out. `docs/parity.md` says what is built,
a test checks that it says so truly, and a copy of that claim kept by hand here
is the same claim without the check. So there is no list.

`Color` stores sRGB-encoded components rather than linear light. `Color::srgb`
keeps what it is given, `Color::linear` encodes on the way in, and `to_array`
hands back what is stored. Anything that decomposed a color and rebuilt it needs
`Color::srgb` on the way back, not `Color::linear` -- rebuilding with the latter
now applies the transfer function a second time. Render targets are plain
unsigned normalized formats for the same reason: an sRGB target would encode
what is already encoded.

`BatchDraw::to_uniform` takes the format it is about to be drawn into. Only the
backend knows that, and it is what decides a gradient's dither; the same
recording drawn into an eight-bit surface and a float one wants different
answers, so it could not have been a property of the recording.

`transform` was on that list too, taking a 2D affine where `dart:ui` takes a 4×4
and so admits perspective, and was described as a deliberate design limit
rather than an omission. That was an assertion with no argument behind it,
unlike the superellipse decision beside it, and it is no longer true either way:
`concat_4x4` takes the same matrix `dart:ui` does.

Two things that a caller writing a runtime effect has to know about, and the
only reason they are not compatibility breaks is that nothing has been
published. An effect declares the paint's uniform block by hand, and `to_local`
grew from one vector to three so that a paint's mapping can carry perspective --
so `params` and everything after it moved. And an effect's program is linked
from this renderer's own vertex stage, whose varying at location zero is now a
three-component homogeneous clip position rather than a two-component one; an
effect reading it divides by the third component to get where it used to be.

Color states which primaries it is against. `Color` carries a `ColorSpace` --
sRGB, extended sRGB, or Display P3 -- and converts between them, matching what
`dart:ui` does; `Color::display_p3` states one exactly, which for a saturated
red means components outside zero to one, because the sRGB primaries describe a
smaller triangle than P3's. Nothing between the paint and the target clamps, so
those components reach a floating-point surface intact, through layers,
gradients and filters.

The GLES backend has the advanced blend modes. It reports
`GL_KHR_blend_equation_advanced` as the capability now rather than refusing the
fifteen modes outright, so the same picture reaches both backends -- 29 of 29
modes are checked against the compositing equations on GLES where 14 were.
Nothing about the API changed; a caller that was refused is not.

A blur's sigma is in the space the drawing is in and scales with the transform,
where it used to be in device pixels. `Paint::with_mask_blur`,
`Layer::with_blur` and `Layer::with_backdrop_blur` all change meaning: the same
number under a scale of three is now three times the blur, which is what
`dart:ui` states and what upstream does. `draw_shadow` divides by the scale to
cancel it, so a shadow's softness is unchanged -- also upstream's arrangement.

`drawPoints` in `PointMode::Points` records one draw where it recorded one per
point. The dots carry their centers on the vertices rather than in the paint, so
they share a material; the edge is unchanged, to the byte. A paint that is not a
single solid color, or one carrying a mask blur or an image filter, keeps the
per-point route.

`drawImageNine` records one draw where it recorded nine. The patches carry
their texture coordinates on the vertices rather than each carrying a source
rectangle of its own, with a half-texel inset standing in for the clamping a
per-draw source rectangle used to give. The picture is unchanged, to the byte.

A rounded superellipse costs a quarter of the geometry it did. The
conic-to-quadratic conversion subdivided until the curve's *weight* was near
one, which is a proxy that doubles its output per step; it now stops when the
approximation error itself is small enough, at the same relative tolerance. The
outline moves by a few thousandths at its tangent extremes, which is four
pixels of a hundred-and-twenty-eight-square frame.

A stroke wider than `MAX_STROKE_WIDTH` draws nothing rather than aborting the
process. `Paint::stroke(color, 1e30)` with a round join used to overflow the
stack inside the tessellation dependency, which no caller can catch.
`MAX_STROKE_WIDTH` is new and public.

A path is bounded before it is tessellated. A coordinate past two to the
twenty-fourth is refused rather than drawn, because past that a float cannot
name a pixel and because a stroke at a larger one grows without bound -- three
verbs at `1e15` used to stroke to thirty-one million vertices. A tolerance that
is not a positive length falls back to the default rather than being passed to
a dependency that asserts on it. `Path::is_within_tessellation_range` and
`MAX_COORDINATE` are new and public.

The workspace denies `unwrap`, `panic!`, `todo!` and `unimplemented!` in
shipping code, and `unsafe_op_in_unsafe_fn` everywhere. Nothing in the public
API changed; a handful of internal `unwrap`s became `expect`s that say what
they are relying on.

A stroked rectangle keeps its join. `draw_rect` and `draw_rrect` at a radius of
zero handed a stroked shape to the fragment-evaluated route, whose stroke band
rounds a vertex whatever the join says, so a rectangular border came out with
rounded corners. They tessellate now unless the caller asked for a round join.

A stroke narrower than a device pixel draws differently. It is widened to one
pixel and dimmed by `clamp(2 * scaled_width, 0, 1)`, which is upstream's
arithmetic with upstream's constants, so a thin line fades with its width
instead of quantizing against the sample grid -- before this a stroke of 0.18
device pixels and one of 0.3 laid down the same ink and one of 0.15 laid down
none. A width of zero still draws nothing, which is the one part of upstream's
rule this renderer does not take and is `docs/parity.md`'s `strokeWidth` row.
`Material::with_opacity` and `StrokeStyle::with_width` are new and public.

A layer says which backdrop it filters. `Layer::with_backdrop_id` is `dart:ui`'s
`backdropId`: layers naming one filter the image captured the first time it was
used rather than each capturing afresh, which is a different picture wherever
they overlap and one capture instead of one per layer. `Layer` gains a public
field for it, so a struct literal naming every field by hand needs the new one;
`Layer::default` and the builders do not change.

Presenting a wide gamut is not built and is not claimed. The swapchain and the
scanout path are untouched, because the devices available for testing are a
software rasterizer and a virtual display controller and a Display P3 surface
cannot be exercised on either.

Two more signature changes, again breaks only because nothing has been
published. `execute_layers` takes the format its intermediates should use, so
that a layer can hold what the frame it composites into holds; `execute` derives
it and none of *its* callers change. And `Context::read` was documented as
returning tightly packed RGBA8 when it has always returned whatever the surface
format packs -- eight bytes per pixel for a floating-point surface, as four
half-floats.

Four defects were found and fixed on the way, none of which needed wide gamut to
be worth fixing. Both sRGB transfer functions compared the signed value against
the knee where the standard means the magnitude, so every negative component
took the near-black linear segment. The GLES backend built a color attachment
for every texture it created regardless of what the caller asked for, which
turns a sampling-only texture into a creation failure for any format that is
filterable and not renderable. Its transfer paths named the channel layout from
the format and the component type from a literal. And a gradient of more than
four stops was tabulated through eight bits, so adding a stop that changed
nothing about a gradient changed the picture by twenty-four levels once a color
filter brought the difference back into view.

## 0.0.0 — 2026-08-22

A placeholder holding the name, containing no API.

Published because `impeller` on crates.io belongs to an unrelated crate that
got there first, so the name this project actually builds under needed
claiming before it went the same way. The version is `0.0.0` so that it sorts
below anything real and `cargo add impeller-rs` will not resolve to it once a
release exists.
