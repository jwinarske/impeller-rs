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

What stands between here and a first release with an API:

- Two operations are absent: `drawRSuperellipse` and `clipRSuperellipse`.

`transform` was listed here too, taking a 2D affine where `dart:ui` takes a 4×4
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
