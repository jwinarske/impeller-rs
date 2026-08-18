# impeller-rs

A tessellation-based 2D vector graphics renderer for Rust, targeting everything
from desktop discrete GPUs down to embedded SoCs driving panels directly
through KMS with no compositor present.

> **Status: early development.** Solid fills, strokes, antialiasing, gradients
> (linear, radial, sweep), and the Porter-Duff blend modes render on both
> backends and are verified against real hardware and a software reference. The
> advanced blend modes — separable and non-separable both, all fifteen — render
> on Vulkan where the device offers the advanced-blend extension, and are
> reported as unavailable elsewhere rather than approximated. Clipping works on both
> backends for rectangles and arbitrary paths, nests, and antialiases with the
> shapes it confines. Images can be uploaded and drawn as a paint, with clamp,
> repeat and decal tiling. Save layers give a subtree its own target, so group
> opacity and layer-wide blend modes work and nest. A glyph atlas packs
> caller-supplied single-channel coverage, evicts what a frame stops using,
> and draws a run of any length as one draw; bring your own rasterizer. Vulkan windowed presentation works through a swapchain
> built on a surface the caller supplies; the GLES window path is not
> implemented yet.

## Why

- **Tessellation-based, no compute shader requirement.** Runs correctly on
  every Vulkan 1.1+ device and every GLES 3.0 device, including embedded GPUs
  where compute is weak or driver support is immature.
- **Predictable frame times.** All pipelines are compiled ahead of time — no
  shader compilation jank, no driver-specific compute paths.
- **First-class direct scanout.** Most 2D renderers assume a windowing system
  exists. Here, rendering straight to a KMS plane with no compositor is a
  supported and tested configuration rather than an exercise left to the
  reader.
- **Pure-Rust source tree.** Every dependency compiles from Rust source. No C
  or C++ toolchain, no pkg-config, no system headers at build time, which keeps
  cross-compilation to aarch64 and riscv64 boards trivial.

## The shape of it

Two orthogonal axes. The **rendering HAL** answers how draw commands become
pixels in a GPU image. **Presentation** answers how a finished image reaches
the display, and how the frame loop is paced. DRM/KMS is a presentation target
alongside windowed surfaces, not a third rendering backend.

|            | WSI (windowed)      | DRM (direct scanout)                          |
|------------|---------------------|-----------------------------------------------|
| **Vulkan** | `VkSwapchainKHR`    | VkImage → dma-buf export → drm-rs FB → commit |
| **GLES**   | EGL window surface  | EGL on GBM → gbm_surface → drm-rs FB → commit |

All four are Tier 1 on Linux. The windowed column works today against a surface
the caller supplies. The scanout column is **partial**: the frame loop above
KMS is implemented and tested — the buffer ring, fence plumbing, and format and
modifier negotiation — and dma-buf export from Vulkan is real. A buffer this
renderer allocates, draws into and exports is accepted by a real display
controller as a framebuffer, the mode is set, and frames flip in turn — checked
against the virtual KMS driver, since a compositor holds master on any card
driving a display. The render fence rides each commit, so the
kernel latches the flip when rendering completes and the frame loop blocks on
nothing — except on a commit that also sets the mode, which vkms will not
complete with a fence attached and which happens once per output.

`cargo xtask drm` says whether a given machine could run that lane.

```rust
use impeller::{BackendPreference, Canvas, Color, Context, Extent2D, Paint, PixelFormat, Rect};

// The backend is chosen at run time, so one binary serves a board with a
// working Vulkan driver and one where only GLES is usable.
let mut ctx = Context::new(BackendPreference::Auto)?;

let size = Extent2D::new(256, 256);
let mut surface = ctx.create_surface(size, PixelFormat::Rgba8Unorm)?;

let mut canvas = Canvas::new(size);
canvas.clear(Color::WHITE);
canvas.draw_rect(
    Rect::new(32.0, 32.0, 224.0, 224.0),
    &Paint::fill(Color::rgba8(0, 120, 220, 255)),
)?;

// Recording is separate from submitting, so a whole frame is described
// before any of it reaches the GPU and every shape shares one pass.
ctx.draw(&mut surface, &canvas.finish())?;
let pixels = ctx.read(&mut surface)?;
```

## Running it

```sh
cargo run -p impeller --example frame -- frame.ppm
```

Draws one frame through the public API — a gradient, a group composited through
a layer, an image sampled through a path clip, a blend mode, and a run of
glyphs — and writes a binary PPM, which needs no encoder. Image encoding is out
of scope, so converting to something friendlier is `magick frame.ppm frame.png`.

## Building

Requires a Rust toolchain; no other build dependencies. MSRV is 1.82.

```sh
cargo build                              # default: vulkan + WSI
cargo build --features gles,drm          # the classic embedded GBM path
cargo build --features vulkan,gles,drm   # one binary that picks at runtime
cargo test
```

`drm` is presentation-only and composes with either rendering backend. Feature
flags live on the `impeller` facade crate, since a virtual workspace root
cannot declare them.

Development tasks — device runs against real boards, golden-image management,
CI reproduction — run through `cargo xtask`. No commands are implemented yet.

## Layout

```
crates/
  impeller              public facade; carries the feature flags
  impeller-core         public API: Canvas, Paint, Path, Image, GlyphRun
  impeller-entity       entity and contents layer
  impeller-geometry     path types, tessellation, analytic fast paths
  impeller-renderer     render pass encoding, generic over the HAL
  impeller-text         glyph atlas, rasterization, SDF
  impeller-hal          rendering HAL trait
  impeller-hal-vulkan   Vulkan backend (first-class)
  impeller-hal-gles     GLES 3.0 backend
  impeller-present      presentation trait, format negotiation
  impeller-present-vk   Vulkan WSI swapchain target
  impeller-present-egl  EGL window-surface target
  impeller-present-drm  DRM/KMS scanout target
  impeller-shaders      WGSL sources, build-time translation via naga
  impeller-testkit      shared test harness
  impeller-capi         Impeller C API (libimpeller), ABI-compatible
xtask/                  device runs, golden management, CI reproduction
docs/                   architecture
```

## Impeller C API

`impeller-capi` builds `libimpeller`, intended as an ABI-compatible
implementation of upstream Impeller's C API so that a consumer linking that API
could link this instead without recompiling. **It is barely started**: version
negotiation is the only entry point, and the rest waits on a vendored copy of
the upstream header — guessing an enum value or a struct layout would produce a
library that links and then corrupts memory.

It is **not** a drop-in for Impeller inside the Flutter Engine build: the
engine compiles Impeller's C++ sources directly rather than consuming them
across this boundary, and no Rust library can present a compatible C++ ABI. The
C API serves embedders. See
[`docs/architecture.md`](docs/architecture.md#impeller-c-api-compatibility) for
where parity is partial and how it is verified.

## Scope

Bring your own text shaping and layout (`cosmic-text`, `parley`), font parsing
(`ttf-parser`, `swash`), image decoding (`image`), and SVG parsing (`usvg`).
Scene graph, retained mode, animation, and 3D are out of scope, as is GLES 2.0.

This is a renderer, not a compositor: it draws to planes it is given.
Multi-client composition belongs elsewhere. KMS internals — connector probing,
EDID parsing, mode selection policy, session management — belong to drm-rs or
the application.

## Documentation

[`docs/architecture.md`](docs/architecture.md) covers the design and the rules
that govern the codebase: the HAL and presentation split, the Vulkan-first
policy, explicit synchronization, the ownership boundary with drm-rs, format
and modifier negotiation, the shader pipeline, dependency purity, and the
testing model. Read it before proposing structural changes — a fair number of
alternatives were considered and rejected for recorded reasons.

## License

BSD 3-Clause. See [`LICENSE`](LICENSE).

This matches the Flutter Engine, home of the C++ Impeller whose architecture
this project takes as its reference. Any code ported from there retains its
original copyright notice alongside this project's.
