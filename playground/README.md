# Playground

Look at a scene in a window, live.

| Key | |
|---|---|
| Right, Space | next scene |
| Left | previous scene |
| Up, Down | turn the current live scene's knob |
| A | toggle animation |
| Q, Escape | quit |

Two kinds of scene. The **corpus** is what the test suite compares — fixed size,
fixed parameters — rendered at its own size and scaled into the window. **Live**
scenes are drawn at the window's size and carry one parameter you can sweep,
which is what a still image cannot show: whether a value moves smoothly through
its range, and whether something that changes every frame changes well.

The gradient scene is the one to try first. Its knob is the number of color
stops, and it crosses the point where they stop fitting in a material and get
baked into a texture instead — two different shader paths. A step in the picture
as it crosses is those paths disagreeing.

The conical gradient is the other one worth sweeping deliberately. Its knob
slides the first circle's center out toward the second's edge and past it. At
exactly one the two circles are tangent, the family of circles collapses to a
half plane, and the quadratic the shader solves loses its squared term — a
branch taken on a set of measure zero, which is the sort of thing a knob finds
and a fixed comparison cannot.

```sh
cd playground && cargo run
```

## Why it is not in the workspace

The workspace bans `cc`, `cmake` and `pkg-config` from its dependency graph,
and checks that on every push. Compiling from pure Rust source is what keeps
cross-compiling the renderer to aarch64 and riscv64 boards trivial.

Every route to a Wayland window in Rust goes through `wayland-backend`, which
has `cc` and `wayland-sys` — and so `pkg-config` — as unconditional build
dependencies. That is true of `winit`, of `wayland-client` with no default
features, and of `winit` with `wayland-dlopen`, which changes only how libwayland
is loaded at runtime and not what the build needs. There is no pure-Rust Wayland
client to reach for.

So this lives outside, with its own lockfile. The rule exists to protect the
renderer's portability, and a desktop program for looking at scenes is not
shipped to a board. Nothing here is visible to a workspace build, to
`cargo deny`, to the minimum-toolchain job or to the cross-compile jobs — which
is the point, and is checked by those jobs continuing to pass with this present.

## What it does not do yet

- **GLES.** Vulkan only so far; the EGL window target exists and the same window
  can carry it, chosen at startup rather than live, since the contexts differ.
- **A board.** The DRM/KMS target needs no window server at all, and running the
  same scene list through it is the version that matters where this renderer is
  aimed.
