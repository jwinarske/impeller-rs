# Playground

Look at a corpus scene in a window, live. Right or Space for the next scene,
Left for the previous, Q or Escape to quit.

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
- **Live parameters.** Scenes render as the corpus defines them. The knobs worth
  having — blur sigma, dash phase, tile mode — come next, on keys.
- **A board.** The DRM/KMS target needs no window server at all, and running the
  same scene list through it is the version that matters where this renderer is
  aimed.
