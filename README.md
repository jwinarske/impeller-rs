# impeller-rs

A tessellation-based 2D vector graphics renderer for Rust, targeting everything
from desktop discrete GPUs down to embedded SoCs driving panels directly
through KMS with no compositor present.

> **Status: early development.** The workspace and crate boundaries exist; the
> renderer does not. Every crate is currently a documented stub, and the test
> suite is empty. Nothing here draws pixels yet.

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

All four combinations are Tier 1 on Linux. The frame loop is identical across
every one of them — sketched below as the intended API, none of which is
implemented yet:

```rust
let ctx = impeller::Context::new(BackendPreference::Auto)?;

// Windowed...
let mut target = impeller::present::window(&ctx, &raw_window_handle)?;
// ...or straight to a display, with no compositor in the picture:
let mut target = impeller::present::drm(&ctx, output)?;

loop {
    let frame = target.acquire()?;          // paced by the target
    let mut canvas = Canvas::for_frame(&ctx, &frame);
    draw_ui(&mut canvas);
    let done = canvas.finish()?;
    frame.present(done)?;
}
```

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
xtask/                  device runs, golden management, CI reproduction
docs/                   architecture
```

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
