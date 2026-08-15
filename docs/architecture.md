# Architecture

impeller-rs is a tessellation-based 2D vector graphics renderer. It targets the
full range of Linux-capable graphics hardware, from desktop discrete GPUs down
to embedded SoCs driving panels directly through KMS with no compositor
present.

This document records the invariants that govern the codebase. Where a rule
here conflicts with local convenience, the rule wins; several of them exist
because violating them is cheap today and a breaking redesign later.

## Positioning

Four properties define the project against neighboring renderers:

- **Tessellation-based, no compute shader requirement.** Runs correctly on
  every Vulkan 1.1+ device and every GLES 3.0 device, including embedded GPUs
  where compute is weak or driver support is immature.
- **Predictable frame times.** All pipelines are compiled ahead of time. No
  shader compilation jank, no driver-specific compute paths.
- **First-class direct scanout.** Most 2D renderers assume a windowing system
  exists. Here, rendering straight to a KMS plane with no compositor is a
  supported and tested configuration rather than an exercise left to the
  reader. It is the configuration embedded, automotive, kiosk, and industrial
  products actually ship.
- **Pure-Rust source tree.** Every dependency compiles from Rust source.

Out of scope, deliberately: text shaping and layout, font parsing, image
decoding, SVG parsing, scene graph and retained mode, animation, 3D. Bring
`cosmic-text` or `parley`, `ttf-parser` or `swash`, `image`, and `usvg`.
Also out of scope: compositor functionality, and KMS internals such as
connector probing, EDID parsing, mode selection policy, and session
management.

## The governing idea: two orthogonal axes

**The rendering HAL answers how draw commands become pixels in a GPU image.
Presentation answers how a finished GPU image reaches the display, and how the
frame loop is paced.**

These axes are independent. Every presentation target works with every
rendering backend that can produce compatible images, and vice versa.

```
┌───────────────────────────────────────────────────────────────┐
│  Public API (impeller-core)                                   │
│  Canvas, Paint, Path, Image, GlyphRun                         │
├───────────────────────────────────────────────────────────────┤
│  Entity layer (impeller-entity)          backend-agnostic     │
├───────────────────────────────────────────────────────────────┤
│  Renderer (impeller-renderer)  — generic over Hal             │
│  RenderPass sorting, pipeline cache, per-frame allocators     │
├───────────────────────────────────────────────────────────────┤
│  Rendering HAL trait (impeller-hal)                           │
│      ┌────────────────┴────────────────┐                      │
│      ▼                                 ▼                      │
│  impeller-hal-vulkan            impeller-hal-gles             │
│  (ash + gpu-allocator)          (glow + EGL)                  │
│  FIRST-CLASS                                                  │
├───────────────────────────────────────────────────────────────┤
│  Presentation trait (impeller-present)                        │
│   ┌──────────────┬──────────────┬───────────────────────┐     │
│   ▼              ▼              ▼                       ▼     │
│ vk-swapchain   egl-window    drm-scanout (Vulkan)  drm-scanout│
│ (WSI)          (WSI)         dma-buf export         (GLES/GBM)│
│                              └───────── drm-rs ─────────┘     │
└───────────────────────────────────────────────────────────────┘
```

**DRM is a presentation target, not a third rendering backend.** The renderer
always draws through a rendering HAL; DRM/KMS is how finished frames reach a
display when no windowing system exists. Any design that treats DRM as a
backend, or that couples a rendering backend to a presentation path, is wrong
by construction.

### Configuration matrix

|            | WSI (windowed)      | DRM (direct scanout)                          |
|------------|---------------------|-----------------------------------------------|
| **Vulkan** | `VkSwapchainKHR`    | VkImage → dma-buf export → drm-rs FB → commit |
| **GLES**   | EGL window surface  | EGL on GBM → gbm_surface → drm-rs FB → commit |

All four cells are Tier 1 on Linux. On non-Linux platforms only the WSI column
applies. Future backends extend the rows, never the columns.

## Rendering HAL

The HAL trait is Vulkan-leaning by design. Recording commands and replaying
them as GL calls works; extracting Vulkan-grade explicitness from an
immediate-mode abstraction does not. GLES pays a small CPU cost for command
recording, and that is the accepted trade.

```rust
pub trait Hal: 'static {
    type Context: HalContext<Hal = Self>;
    type CommandBuffer: HalCommandBuffer<Hal = Self>;
    type Pipeline: Send + Sync;
    type Buffer: Send + Sync;
    type Texture: Send + Sync;
    type Sampler: Send + Sync;
    type Fence: HalFence;
}

pub trait HalContext: Send + Sync {
    type Hal: Hal;
    fn create_command_buffer(&self) -> <Self::Hal as Hal>::CommandBuffer;
    fn create_pipeline(&self, desc: &PipelineDescriptor)
        -> Result<Arc<<Self::Hal as Hal>::Pipeline>>;
    fn create_buffer(&self, desc: &BufferDescriptor)
        -> Result<<Self::Hal as Hal>::Buffer>;
    fn create_texture(&self, desc: &TextureDescriptor)
        -> Result<<Self::Hal as Hal>::Texture>;
    fn submit(&self, cmd: <Self::Hal as Hal>::CommandBuffer)
        -> <Self::Hal as Hal>::Fence;
    fn capabilities(&self) -> &Capabilities;
}
```

### Vulkan-first policy

Vulkan is the reference implementation and the model the HAL trait is shaped
around. Concretely:

1. **Trait evolution follows Vulkan.** When a renderer feature needs HAL
   surface, the trait expresses it the way Vulkan does. Other backends
   implement, emulate, or capability-gate it. The trait is never reduced to a
   lowest common denominator.
2. **Vulkan is the conformance oracle.** Every other backend's output is
   diffed against Vulkan on lavapipe, the deterministic reference, not against
   its own baselines. A cross-backend difference is a bug in the non-Vulkan
   backend until proven otherwise.
3. **Features land on Vulkan first.** New materials, filters, and
   optimizations ship on Vulkan and are gated as unimplemented elsewhere until
   ported. A lagging backend never blocks a Vulkan merge.
4. **Performance is defined on Vulkan.** Other backends get ratio targets
   relative to Vulkan on the same hardware.
5. **Merge-blocking CI is Vulkan.** Other backends' lanes start nightly and are
   promoted to merge-blocking only after a sustained stability period.

### Two requirements that must exist from day one

Both exist for the sake of the DRM path, and both must be in the trait before
any DRM code is written. Retrofitting either one is a breaking redesign.

1. **External render targets.** The renderer must draw into images it did not
   allocate — GBM-allocated buffers imported into Vulkan or GLES, or its own
   allocations exported as dma-bufs. `TextureDescriptor` carries an
   `external: Option<ExternalImageDesc>` variant with dma-buf fd, DRM fourcc,
   and format modifier.
2. **Exportable sync.** `HalFence` must convert to a `sync_file` fd where the
   platform allows it, so the render-done fence can be attached to an atomic
   commit. Where unavailable, the DRM target falls back to a CPU-side wait
   before commit — correct, slower, and reported in capabilities.

### Capability-based branching

`Capabilities` reports max texture size, MSAA sample counts, dma-buf import and
export support, format-modifier support, sync-fd support, and backend feature
level. **Tests and the presentation layer branch on capabilities, never on
backend identity.**

## Backends

### Vulkan (`impeller-hal-vulkan`)

Direct `ash`, no abstraction layer above the HAL. Vulkan 1.1 floor, with 1.3
dynamic rendering, timeline semaphores, and sync2 used when present. Features
avoided for MoltenVK compatibility: geometry shaders, tessellation shaders,
sparse residency, multi-draw-indirect-with-count — none are needed for 2D.

Memory via `gpu-allocator`, with per-frame ring allocators for vertex, index,
and uniform data. Long-lived resources are `Arc`-tracked and retired by the
fence waiter. Pipelines are compiled at context creation from embedded SPIR-V,
with the pipeline cache persisted to disk.

Extensions required on the Linux DRM path: `VK_EXT_external_memory_dma_buf`,
`VK_KHR_external_memory_fd`, `VK_EXT_image_drm_format_modifier`, and
`VK_KHR_external_fence_fd` / `VK_KHR_external_semaphore_fd`.

On multi-GPU and headless boards, physical devices are matched to the DRM node
via `VK_EXT_physical_device_drm`, so rendering happens on the GPU that owns or
can share buffers with the display controller. On split render/display SoCs —
common on ARM, where the GPU is a render-only node and the display controller
is a separate KMS device — scanout buffers are allocated with modifiers both
devices accept. This negotiation is a first-class code path, not an edge case.

### GLES (`impeller-hal-gles`)

GLES 3.0 floor via `glow`, with contexts from EGL in all configurations.
Commands are recorded into a vec and replayed as GL calls at submit, with a
state cache to avoid redundant binds. Programs are linked at context creation
from embedded GLSL ES 300 and cached via `GL_OES_get_program_binary` where
available.

`glFenceSync` and `glClientWaitSync` back `HalFence`;
`EGL_ANDROID_native_fence_sync` exports sync fds for the DRM path. Uniform data
lives in UBOs with std140 layouts generated alongside the shaders; there is no
push-constant equivalent, so small per-draw data uses a ring-buffered UBO with
dynamic offsets. MSAA uses multisampled renderbuffers with a blit resolve, and
`GL_EXT_multisampled_render_to_texture` on tilers where present.

GLES 2.0 is permanently out of scope; the feature gap is too large.

## Presentation

Presentation owns pacing. WSI targets pace via swapchain acquire semantics; DRM
targets pace via page-flip completion events. The renderer never blocks on
presentation internals — it renders into whatever image the target hands it.

```rust
pub trait PresentTarget {
    type Frame: PresentFrame;

    /// Block or async-wait until a frame slot is available, per the target's
    /// pacing policy: mailbox or fifo for WSI, an N-buffered flip queue for DRM.
    fn acquire(&mut self) -> Result<Self::Frame>;

    /// Current output geometry. May change on hotplug or resize.
    fn extent(&self) -> Extent2D;
    fn format(&self) -> PixelFormat;

    /// Called when the target reports a geometry change. Rebuilds images.
    fn reconfigure(&mut self) -> Result<()>;
}

pub trait PresentFrame {
    /// The render-target image for this frame, in HAL terms.
    fn render_target(&self) -> RenderTargetHandle;

    /// Submit for display. `render_done` is the sync primitive the renderer
    /// signals when GPU work completes.
    fn present(self, render_done: SyncHandle) -> Result<()>;
}
```

The offscreen target is a first-class citizen, not a test affordance: the
entire golden and conformance apparatus runs on it.

### Explicit sync is the design center

On the DRM path the render-done primitive is exported as a `sync_file` fd and
attached to the atomic commit as `IN_FENCE_FD`; the kernel latches the flip
when the fence signals. On WSI paths it is the ordinary swapchain semaphore.

**No `glFinish` and no `vkDeviceWaitIdle` in the frame loop on any path.**
Where a driver lacks fence export, the CPU-wait fallback is capability-gated
and logged loudly. It is treated as a per-driver bug to chase, never silently
accepted on Tier-1 hardware.

### Format and modifier negotiation

Presentation targets advertise `(fourcc, modifier[])` sets; the HAL context
advertises what it can render to and export. The intersection is chosen from,
preferring non-linear vendor modifiers when both sides accept them and falling
back to `DRM_FORMAT_MOD_LINEAR` only when necessary.

**Negotiation failure is a hard error** with both sides' sets dumped, and the
chosen modifier is always logged. A silent linear fallback halves memory
bandwidth on an embedded panel, so it is a bug rather than a graceful
degradation.

### DRM/KMS direct scanout (`impeller-present-drm`)

Built on drm-rs, the Rust port of drm-cxx. **The dependency direction is
one-way and this crate reimplements no KMS logic.**

| Concern | Owner |
|---|---|
| Device open, master acquisition, seat handoff | drm-rs, plus app or session manager |
| Connector, CRTC, and plane discovery; mode selection | drm-rs |
| Atomic commit construction, page-flip events, hotplug | drm-rs |
| dma-buf to framebuffer import | drm-rs |
| HDR metadata, VRR, plane rotation properties | drm-rs |
| Buffer allocation, image import and export | impeller-present-drm |
| Frame pacing against flip completion, fence plumbing | impeller-present-drm |

**Vulkan path**, triple-buffered: a ring of VkImages is allocated with
`VK_EXT_image_drm_format_modifier` using the negotiated modifier set and
exported once at startup as dma-bufs, which drm-rs imports into framebuffers.
All of that is setup, not per-frame. Each frame then acquires a ring slot whose
previous flip has completed and whose GPU fence has retired, renders, exports
the signal semaphore as a sync_file, and issues a nonblocking commit. Where
`VK_EXT_image_drm_format_modifier` is absent, the fallback is GBM allocation
plus Vulkan dma-buf import. Both paths are supported and tested; capability
detection picks between them.

**GLES path**, the classic GBM route: a `gbm_device` on the DRM fd backs an EGL
display on the GBM platform, with a `gbm_surface` created against the
negotiated modifier set. Each frame renders, swaps, locks the front buffer,
imports it (cached — GBM recycles a small ring internally), commits, and
releases the previous buffer on flip completion.

Pacing is flip-event driven with configurable acquire depth. Hotplug and
modeset surface as a reconfigure error, and the target rebuilds its buffer ring
against the new mode. Multi-display means one target per output, rendered
independently; cloned versus extended policy belongs to the application.

For panels mounted rotated, the KMS plane rotation property is used when the
hardware supports the needed rotation for the chosen format and modifier;
otherwise the renderer pre-rotates via a transform on the root canvas. The
choice is capability-driven.

## Threading

- `Context` is `Send + Sync`; resource creation is thread-safe.
- `Canvas` recording is single-threaded per frame.
- A background fence waiter retires GPU work and releases tracked resources,
  handling Vulkan fences, `GLsync` objects, and sync_file fds uniformly.
- DRM page-flip events arrive on drm-rs's event dispatch; the DRM target runs a
  small event thread converting flip completions into frame-slot availability.

## Shader pipeline (`impeller-shaders`)

**One WGSL source tree is the single source of truth.** `build.rs` runs naga to
produce SPIR-V for Vulkan and GLSL ES 300 for GLES, with MSL, HLSL, and desktop
GLSL available for future backends from the same source and no C++ shader
toolchain anywhere in the build.

Hand-written overrides live in a per-backend directory for cases where naga
output is wrong or slow, and **the build fails if an override goes stale
against its WGSL twin**. A derive macro generates matching Rust structs, std140
UBO layouts, and vertex input descriptions from one definition, so layout
mismatches are compile errors.

naga output for every shader and target is snapshotted in the repository and
diffed in CI, so a naga upgrade that changes codegen is a reviewed event rather
than a silent behavior change. Specialization maps from one set of declarations
in the WGSL source: spec constants on Vulkan, bounded build-time macro
permutations on GLES.

## Renderer internals

- **Geometry** (`impeller-geometry`): lyon for general fills and strokes;
  analytic fast paths for rect, rrect, circle, and ellipse that compute
  coverage in the fragment shader instead of tessellating; convexity detection
  for a fan-fill fast path; Wang's-formula adaptive Bezier flattening with
  transform-aware scale.
- **Entity layer** (`impeller-entity`): an entity carries transform, blend,
  clip depth, contents, and geometry, with a Contents implementation per
  material and coverage computation for culling.
- **Passes** (`impeller-renderer`): draw commands are buffered per pass, sorted
  by pipeline, and encoded once at pass end. Save layers become offscreen
  targets with a paint-composited restore; path clipping is stencil-based;
  blur is multi-pass separable.
- **Text** (`impeller-text`): swash rasterization into LRU atlas pages,
  quarter-pixel subpixel quantization, SDF above a threshold size, and
  COLR/CPAL color glyphs as image quads.
- **Color**: linear f32 internally, with sRGB conversion at the API boundary
  and at target write.

## Crate layout

| Crate | Owns |
|---|---|
| `impeller` | Public facade; carries the feature flags |
| `impeller-core` | Public API: Canvas, Paint, Path, Image, GlyphRun |
| `impeller-entity` | Entity and Contents layer |
| `impeller-geometry` | Path types, tessellation, fast paths |
| `impeller-renderer` | Render pass encoding, generic over the HAL |
| `impeller-text` | Glyph atlas, rasterization, SDF |
| `impeller-hal` | Rendering HAL trait |
| `impeller-hal-vulkan` | Vulkan backend |
| `impeller-hal-gles` | GLES 3.0 backend |
| `impeller-present` | Presentation trait, shared types, negotiation |
| `impeller-present-vk` | Vulkan WSI swapchain target |
| `impeller-present-egl` | EGL window-surface target |
| `impeller-present-drm` | DRM/KMS scanout target |
| `impeller-shaders` | WGSL sources and build-time translation |
| `impeller-testkit` | Shared test harness |
| `xtask` | Device runs, golden management, CI reproduction |

Feature flags live on the `impeller` facade because a virtual workspace root
cannot declare them. `drm` is presentation-only and composes with either
backend:

```
--features vulkan,drm        Vulkan rendering with KMS scanout
--features gles,drm          the classic embedded GBM path
--features vulkan,gles,drm   one binary that picks at runtime
```

## Dependency purity

**Build time: every dependency compiles from pure Rust source.** No C or C++
toolchain, no pkg-config, no system headers. `cargo build` with any feature
combination needs only a Rust toolchain, which is what keeps cross-compilation
to aarch64 and riscv64 boards and containerized CI builds trivial. This is
enforced by `deny.toml`, which bans `pkg-config`, `cmake`, and `cc` outright.

The known footgun is `khronos-egl`'s `static` feature, which pulls in link-time
libEGL via pkg-config. It is pinned to `dynamic`, which dlopens instead.

Runtime is a different question — the renderer must ultimately talk to GPU
drivers and the kernel, which are C. What matters is the split between what the
project ships and what the OS provides:

| Cell | Non-Rust runtime components | Provider |
|---|---|---|
| Vulkan + WSI | Vulkan loader and driver | OS / GPU vendor |
| Vulkan + WSI (Apple) | MoltenVK | **Bundled by us** |
| **Vulkan + DRM** | Vulkan driver only — no userspace display library at all | OS / GPU vendor |
| GLES + WSI | libEGL, libGLESv2 (dlopen'd) | OS / Mesa / vendor |
| GLES + DRM | libEGL, libGLESv2, libgbm (dlopen'd) | OS / Mesa / vendor |

**Vulkan + DRM is the purest cell in the matrix.** drm-rs speaks raw ioctls to
the kernel with no libdrm, and Vulkan allocates scanout buffers itself, so
there is no GBM. Kernel, Vulkan driver, and pure Rust the whole way up. For
embedded products wanting the smallest auditable userspace, this is the
flagship configuration.

GBM is required only by the GLES + DRM cell. A pure-Rust GBM is not realistic —
Mesa's embeds per-driver allocation heuristics that would have to be
reimplemented and kept in sync — so `libgbm.so.1` is dlopen'd, loaded only when
that cell is instantiated. MoltenVK remains the only non-Rust component the
project itself ships, on Apple platforms only.

## Testing model

**Tests are part of the phase.** A phase without its test suites merged is not
done. There is no end-of-project testing phase.

**One corpus, many executions.** Scenes are data — a versioned serialized IR of
canvas calls — not Rust code. A single corpus drives golden, conformance,
performance, and on-device runs across every backend and presentation
combination. New features add scenes once and the matrix multiplies coverage
automatically. Every rendering bug fixed adds a regression-pin scene, and that
set grows monotonically.

`impeller-testkit` provides the executors (offscreen, WSI, DRM), the
comparators (per-channel tolerance with an outlier budget, perceptual
comparison where cross-driver float variance is expected, CRC equality for
scanout), and a single report schema emitted identically by CI containers and
by a shell on an embedded board.

**Tolerance tables are checked in**, keyed by scene class, backend, and driver.
Tightening one is a normal change; **loosening one requires a linked
driver-bug issue**.

| Level | What | Where |
|---|---|---|
| L0 | Unit: math, path ops, atlas packing, negotiation logic | Every merge, no GPU |
| L1 | Property: tessellation invariants, Bezier tolerance, stroke under transform | Every merge, no GPU |
| L2 | Golden: corpus to offscreen render, image compare | Every merge on software GPU |
| L3 | Conformance: same corpus, cross-backend and cross-presentation diffs | Every merge (software) |
| L4 | Presentation: resize storms, flip pacing, fence ordering, hotplug | VKMS and headless WSI in CI |
| L5 | Stress and soak: atlas thrash, layer-depth bombs, leak detection | Nightly and weekly, hardware |
| L6 | Performance: micro and full-frame benches with regression gating | Nightly, quiet runners |
| L7 | Fuzz: path data, scene descriptions, dma-buf negotiation | Continuous background |

### VKMS gives the DRM path merge-blocking coverage

VKMS provides CI with a real KMS device — atomic modesetting, vblank
simulation, writeback connectors, and CRTC CRC — with no display hardware.
Writeback capture validates the *entire* path (negotiation, dma-buf import,
atomic commit, fence latch), not just the rendered image. CRCs cross-check that
what was committed is what scanned out. Deterministic vblanks make pacing
assertions possible, including proof that the buffer ring never reuses an
in-flight framebuffer. Hotplug injection and failure injection (never-signaling
in-fence, premature framebuffer destroy, mid-stream mode change) drive the
error paths.

Rendering under VKMS uses lavapipe or llvmpipe since VKMS is display-only. That
is exactly the split render/display topology of ARM SoCs, so CI incidentally
exercises the cross-device dma-buf path on every merge.

VKMS proves protocol, not hardware quirks — IOMMU faults, AFBC corner cases,
scaler limits. That is what the board rack exists for. VKMS green with hardware
red is an expected and useful signal, never argued away.

### Flake policy

**Zero retries on merge-blocking lanes.** Retries hide the fence and flip races
this code is most exposed to. A flaky test is a bug against the test itself: it
moves to a non-blocking quarantine lane with a filed issue, and prolonged
quarantine occupancy escalates. Nightly hardware lanes allow one retry, since
real drivers do occasionally hiccup, but every retry is logged so patterns
surface.

### Determinism is engineered

Fixed seeds, serialized scene descriptions, pinned Mesa and kernel versions in
containers, and per-cell tolerance profiles. A container image bump is a change
that re-baselines tolerances under review, never ambient drift.

## Impeller C API compatibility

`impeller-capi` builds `libimpeller`, an ABI-compatible implementation of
upstream Impeller's C API (`impeller/toolkit/interop/impeller.h`). The goal is
binary compatibility: a consumer linking the upstream C API can link this
instead without recompiling. The shared BSD 3-Clause license is what makes
vendoring the header and upstream's test assets clean.

**This is not a drop-in for Impeller inside the Flutter Engine build.** The
engine does not consume Impeller across this boundary — it compiles the C++
sources directly against internal classes carrying templates, STL types, and
virtual inheritance. Rust cannot present a compatible C++ ABI, and inline code
already compiled into the engine's own translation units cannot be replaced by
swapping a library. Rendering Flutter content would additionally require an
engine-side shim dispatching DisplayList calls into this C API, carried as an
engine fork. The C API serves embedders, and that is the seam this crate
targets.

### Conventions the header imposes

- **Reference counting.** Every handle has `Retain` and `Release` entry points,
  both NULL-safe no-ops.
- **No error channel.** Creation returns NULL on failure, operations return
  `bool`. There are no error codes, so diagnostics go to the log. This is a
  narrower contract than the internal `Error` type and information is lost at
  the boundary.
- **Version negotiation.** The caller passes its compiled-in version to context
  creation and a mismatch fails the call, so the implemented version must track
  the pinned header exactly.

### Where parity is partial

Three parts of the C API sit outside what the renderer otherwise commits to.
None is a reason to abandon compatibility, but each is a deliberate exception
rather than an oversight:

1. **Typography.** The header exposes `ImpellerTypographyContext`,
   `ImpellerParagraphBuilder`, and paragraph drawing. Text shaping and layout
   are otherwise explicitly out of scope, on the reasoning that callers bring
   their own shaper. Implementing this surface means the C API layer — not the
   renderer — depends on a shaper, and that dependency is confined to
   `impeller-capi` so the core stays shaper-agnostic.
2. **Runtime shaders.** `ImpellerFragmentProgram` loads shader programs at
   runtime, which is in tension with compiling every pipeline ahead of time to
   avoid compilation jank. Supporting it means accepting a runtime compilation
   path that the renderer's own materials never use, with its cost documented
   rather than hidden.
3. **Backend coverage.** The header offers OpenGL ES, Metal, and Vulkan context
   creation. Metal is a deferred backend, so `ImpellerContextCreateMetalNew`
   returns NULL until it exists. Parity is a subset until then, and the gap is
   reported rather than papered over.

Note also that the C API is WSI-shaped — wrapped framebuffers, drawables, and a
Vulkan swapchain. Direct scanout has no expression in it, so the DRM path
remains reachable only through the Rust API.

### Verifying parity

Symbol-level parity is checked mechanically rather than maintained by hand: CI
diffs exported symbols against a pinned copy of the upstream header, so an
upstream addition surfaces as a failure naming the missing entry points.
Semantic parity — blend mode values, fill rules, color handling, stroke
geometry — is checked by running upstream's C API samples against this library
and comparing output through the usual golden comparators. Semantics, not
symbols, are the hard half.

## Future backends

Metal, desktop OpenGL, D3D12, and WebGPU are planned behind the same HAL trait.
None ships before Vulkan, GLES, and DRM reach 1.0, and each requires
demonstrated demand before work starts.

The 1.0 architecture already accommodates them: the trait was reviewed against
their command models, naga emits their shader targets from the existing WGSL
tree, and the corpus and conformance harness are backend-parametric. Each
activated backend adds a HAL crate, a shader snapshot directory, a software or
reference CI lane, a tolerance profile, and nightly hardware lanes.

| | WSI | DRM scanout | Browser canvas |
|---|---|---|---|
| Vulkan (first-class) | yes | yes | — |
| GLES | yes | yes | — |
| Metal (future) | CAMetalLayer | — | — |
| Desktop GL (future) | EGL | GBM (inherited) | — |
| D3D12 (future) | DXGI | — | — |
| WebGPU (future) | — | — | yes |

Desktop GL is the only future backend that participates in the DRM column, and
it matters: some embedded and legacy industrial stacks expose desktop GL rather
than GLES over their KMS drivers.

Each shipped backend permanently multiplies the conformance matrix, the shader
snapshot set, and the capability-gating surface. The Vulkan-first policy caps
what any additional backend can cost core development, and a backend that loses
its constituency can be demoted to community-maintained or frozen.
