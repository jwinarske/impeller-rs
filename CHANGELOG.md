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

- Two operations are partial against `dart:ui`. `transform` takes a 2D affine
  where `dart:ui` takes a 4×4 and so admits perspective, which is a deliberate
  design limit rather than an omission. `drawAtlas` takes no blend mode for
  combining a sprite's color with its texels, which needs the blend set written
  into the shader, since advanced blending here is the hardware's.
- Two operations are absent: `drawRSuperellipse` and `clipRSuperellipse`.

## 0.0.0 — 2026-08-22

A placeholder holding the name, containing no API.

Published because `impeller` on crates.io belongs to an unrelated crate that
got there first, so the name this project actually builds under needed
claiming before it went the same way. The version is `0.0.0` so that it sorts
below anything real and `cargo add impeller-rs` will not resolve to it once a
release exists.
