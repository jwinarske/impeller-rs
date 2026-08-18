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

**The scanout path runs end to end against a real KMS device.** A buffer this
renderer allocates, draws into and exports as a dma-buf is accepted by a display
controller as a framebuffer, the mode is set, and frames flip in turn. What
makes that checkable is the virtual KMS driver: only one process may be master
of a card and a compositor holds it on any card driving a display, so the
alternative was untested modesetting code.

Three things the recording stand-in had hidden showed up the moment a real
device was on the other end, and each is a defect rather than a surprise.

A display controller takes **one flip at a time**, which is a smaller number
than the ring depth and a different thing from it. The ring bounds buffers in
flight so the renderer can work ahead; commits have to be bounded separately,
and committing while a flip is outstanding is refused by the kernel rather than
queued. The wait sits immediately before the commit, not in `acquire`, because
putting it there would stop the renderer working ahead — which is what the ring
is for.

Reading events from a **blocking** device fd waits until one arrives, so a
`poll_events` documented not to block would block forever the first time
nothing had happened. The device is opened non-blocking for that one caller.

And `present` took the frame's fence out of its slot before a step that could
fail, so an error dropped the fence un-retired and teardown then freed a buffer
the GPU was still reading. The validation layer named it; nothing about the
output would have.

The half of it that needs no privilege now exists and is real. Enumerating
connectors, modes and planes, and reading the format and modifier lists a plane
advertises, all work on a card another process is master of — which is what
makes them checkable on an ordinary desktop. `DrmDevice` does that, and is
deliberately not a `ScanoutOutput`: a type implementing half of that trait would
be one whose other half fails at run time.

Two things about reading a plane are worth knowing before doing it again.
`IN_FORMATS` has to be parsed by hand, because nothing in the crate graph does;
its modifier entries carry a mask whose lowest bit means "the format at this
entry's offset" rather than "the first format", so reading the offset as zero
attributes layouts to formats that never claimed them. And a plane list comes
back holding only overlay planes unless the universal-planes client capability
is set first — the symptom is a device that appears to have no primary plane at
all.

What that buys immediately is a negotiation checked against hardware rather than
against a list a test wrote. Both halves used to come from the same place; the
display's half is now what a display advertises, and on this machine the two
agree on a tiled layout rather than falling back to linear.

**The fence rides the commit, and that is demonstrated.** It is attached as
`IN_FENCE_FD` so the kernel latches the flip when rendering completes rather
than the caller blocking until it has, and a run of frames against a real
device blocks on nothing.

With one exception, which is a real constraint rather than a caveat: **a commit
that also sets the mode does not carry a fence.** The virtual KMS driver never
completes a flip for one that does. A modeset happens on the first frame and
after a hotplug, so waiting on the CPU there costs one stall in the life of an
output, and the alternative was a path demonstrated by nothing.

Finding that cost a detour worth recording, because the first suspicion was
wrong. The fence looked like the culprit, and the Vulkan suite turned out to be
able to export a sync_file and check it outlived its fence while never checking
that it *signals*. It does; that check now exists; and the fault was the
pairing with a modeset rather than either half alone.

The count of CPU waits is what a test asserts against, because it is the only
thing that distinguishes the explicit path from the fallback: eight frames
report one wait, and a build that quietly stopped attaching the fence would
report eight.

`cargo xtask drm` reports whether a machine can host this lane at all, and what
its primary plane would accept.

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
    type Texture: HalTexture;
    const NAME: &'static str;
}

pub trait HalContext {
    type Hal: Hal;
    fn capabilities(&self) -> &Capabilities;
    fn create_texture(&mut self, desc: &TextureDescriptor)
        -> Result<<Self::Hal as Hal>::Texture>;
    fn destroy_texture(&mut self, texture: <Self::Hal as Hal>::Texture);
    fn submit_batch(
        &mut self,
        target: &mut <Self::Hal as Hal>::Texture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<()>;
    fn read_texture(&mut self, texture: &mut <Self::Hal as Hal>::Texture) -> Result<Vec<u8>>;
}
```

### Batches, not command buffers

An earlier shape of this trait had callers record incrementally — begin a pass,
bind a pipeline, draw, finish — mirroring how Vulkan itself works. Building a
real backend showed that to be the wrong seam, and the trait was revised to
take a whole `Batch`: shared geometry plus a list of draws over it.

The decisions a backend actually wants to make are global to a batch. Which
draws can share a pipeline binding, how to lay out one shared vertex buffer,
what to upload in a single copy — none of those are answerable one call at a
time. Handing over a stream forces every backend to reconstruct the shape, and
a record-and-replay backend has to buffer the stream anyway just to see what it
was given.

This keeps the explicitness that matters. Nothing is discovered at draw time
and a caller states its whole intent up front; what changes is that realizing
that intent is the backend's business. The rule that the trait follows Vulkan
rather than being reduced to a lowest common denominator is unchanged — this is
Vulkan's model raised one level, not a concession to a weaker backend.

**Framebuffer orientation needs no correction on readback.** The two APIs
number framebuffer rows from opposite ends, and the usual consequence is an
image flipped on one of them. Shader translation already negates Y per target,
which cancels the difference exactly: a read comes back identically oriented
from either backend, and adding a row flip would reintroduce the mirror rather
than remove it. Both backends assert this against a scene that is not
symmetric top-to-bottom, since a symmetric one proves nothing.

**Blend factors live in one table, not one per backend.** The Porter-Duff set
is expressible with fixed-function factors, so it works on every device with no
extension and no capability gate — which is why it comes first. Each backend
translates portable factors rather than restating the table, so the two cannot
disagree about what a mode means. All of them assume premultiplied colour, and
alpha uses the same factors as colour: with premultiplied colour the alpha
channel is not a special case, and giving it different factors is what breaks
compositing a layer onto something else.

The separable modes — multiply, screen, overlay and the rest — mix the two sides
arithmetically rather than deciding where each survives, and the non-separable
four — hue, saturation, color and luminosity — exchange whole attributes of a
color, so each output channel depends on all three inputs. Neither is expressible
as blend factors. They need an advanced-blend extension and are
capability-gated. A renderer that could not composite at all without one would
be unusable on the hardware least likely to have it, which is why the
fixed-function set came first. `BlendMode::factors` returns an option rather
than a plausible pair for these, so a backend cannot silently render one as
something close: a wrong blend mode is a picture nobody can debug from, and the
check that refuses it lives in `Capabilities` so both backends refuse the same
batch for the same reason.

Their formulas are fixed by the compositing specification, and it is transcribed
once in the HAL. The non-separable four are checked against *what they are named
for* rather than against values — whether hue kept the backdrop's luminosity,
whether saturation took the source's — which is both a stronger statement and
one that reads as the definition it is.

Two things about testing them against hardware were learned by getting them
wrong. The composite scales a blend function's contribution by the product of
the two alphas, so with both sides half transparent a wrong formula arrives at
the target attenuated to a third, and a two-percent error in a luminosity landed
inside the one-unit tolerance and passed. Near-opaque sides recover the
sensitivity while still exercising the premultiplied round trip. And the inputs
have to stay clear of where the formulas turn — dodge and burn clamp at a ratio
of one, hard-light at a half, soft-light at a quarter — because a channel
sitting on one of those is a knife edge where the hardware's un-premultiply and
this one's need differ only in their last bit to fall on opposite sides. The conformance tests check the hardware against that
transcription; each backend's mapping from mode to blend op shares no code with
it, which is what makes checking one against the other mean something. The
transcription is not a second implementation for the sake of testing — a
software path would evaluate the same function.

**Advanced blending is the first genuinely Vulkan-first feature.** Vulkan gets
it through `VK_EXT_blend_operation_advanced`, gated on all operations being
present, the coherent feature being available, and the attachment limit covering
what a pass binds — reported as one flag, since a caller can do nothing useful
with two of the three. Coherency is not taken on the driver's word: a test
renders overlapping draws as one batch and as two submissions, which differ only
if the second draw blends against a stale destination.

GLES has `GL_KHR_blend_equation_advanced`, but reaching it needs a hand-written
GLSL ES fragment stage. The extension requires the shader to declare
`layout(blend_support_all_equations) out;`, and naga's GLSL backend cannot emit
that qualifier from WGSL — this is the "hand-written overrides where translation
falls short" path, and it is the first thing to need it. Where the coherent
variant of the extension is absent, it additionally needs `glBlendBarrierKHR`
between overlapping draws, which changes how a batch is recorded rather than
merely which enum is set. Until that lands, the GLES backend reports the
capability as false and refuses the modes.

That asymmetry is why the scene corpus derives what a scene *requires* from what
it contains, alongside deriving its tolerance. A scene refused by a device that
declares it needs nothing special is a defect; a scene refused by a device the
scene says cannot render it is a declared gap, and the corpus reports the second
as coverage it did not get rather than as a pass. Deriving rather than declaring
matters for the same reason it does for tolerance: a requirement written
alongside a scene is one that can be forgotten, and a forgotten one turns a
known gap into a reported regression. Scenes are run on the first available
device that supports them rather than only on the preferred one, since the
extension can sit on a device the preference order does not pick.

**Clipping starts with the scissor, and that is not a placeholder.** An
axis-aligned rectangle in device pixels is expressible directly through the
fixed-function scissor unit on every device, exactly and at no cost. That stays
true once stencil-backed clipping exists, so an axis-aligned clip should keep
using it rather than paying for a stencil pass. The HAL type is named `Scissor`
for that reason: a clip in a drawing API is whatever region a caller asked for,
and only some of those are rectangles.

`Canvas::clip_rect` intersects rather than replaces, so a subtree can only
narrow what its parent allowed, and the clip saves and restores alongside the
transform in one stack — letting the two unwind independently would let a caller
balance one while leaving the other adrift. A clip is fixed in device pixels at
the moment it is applied, so a later transform moves the shapes drawn inside it
without moving the clip.

Where the current transform rotates or skews by anything but a quarter turn, a
rectangular clip stops being a rectangle and goes through the stencil instead.
Taking its bounding box would admit pixels the caller asked to remove. The
quarter-turn case stays on the scissor because a right angle keeps a rectangle
rectangular; the test for that is relative rather than exact, since `cos` of a
right angle in `f32` is about `-4.4e-8` and an exact comparison would refuse
precisely what the caller asked for.

**The stencil holds a clip's nesting depth, not a mask of which clips apply.**
The obvious encoding gives each clip a bit, which caps nesting at eight and
makes intersecting two clips a per-bit affair. A depth fits a stack of any size
in the same eight bits and reduces the test to one comparison: content at depth
`d` draws where the stencil holds `d`, which is true only where every clip down
to that depth admitted the pixel. It also makes undoing a clip local — because a
stack unwinds in the order it was built, no pixel can hold more than the depth
being left, so stepping back is a decrement rather than a recomputation from the
clips that remain.

Clip geometry goes through the same fill tessellator as a drawn shape, which
matters for more than consistency: the result is non-overlapping triangles, so
the stencil steps forward once per covered pixel and needs no parity trick. Clip
draws mask off every color channel rather than relying on a blend mode that
happens to discard the source, so clip geometry cannot touch the target however
exotic the blending is.

Depth beyond 255 is refused rather than wrapped. Eight bits is the only stencil
depth every device is required to offer, and past it the value returns to zero
and the clip admits everything it was meant to exclude — a wrong picture rather
than an error, which is why it is caught before recording.

Restoring a scissor is an assignment; restoring a stencil clip is a draw, one
per level being left. Those draws are deliberately unscissored: a step back
lands exactly where its matching narrowing landed, because that is where the
stencil holds the value being tested, so making it agree with a scissor that has
already been restored to something else would only be a way to get it wrong.

The two mechanisms are independent and both apply. That is what keeps the
scissor from being a stepping stone: an axis-aligned clip stays on it even
inside a path clip, because a scissor is exact and free where narrowing the
stencil costs a draw.

The stencil attachment is transient — cleared at pass start, discarded at the
end, never stored — so a clip stack is built and unwound entirely within one
pass and a tiler keeps it in tile memory. It carries the color attachment's
sample count, which is also what antialiases a clip edge: with a per-sample
stencil, a boundary crossing a pixel admits some of its samples and not others.

A pixel belongs to a clip when its center does — the same rule the rasterizer
applies to the shape being drawn — so a shape and a clip along the same edge
agree about which pixels lie on it. Rounding outward would admit pixels the
caller excluded; rounding inward would drop ones they kept.

**Neither backend converts the scissor's vertical axis**, which is the opposite
of what the two APIs' conventions suggest. OpenGL numbers window rows upward
from the bottom, so a conversion looks obligatory; it is not, because the shader
translator already negates Y for GLSL and geometry lands in the GL framebuffer
with its top row at GL's zero. That is the same cancellation that makes reading
a target back need no row flip, and converting anyway mirrors the clip for
exactly the reason an added row flip mirrors the image. Both mistakes are
invisible in a target symmetric about its horizontal center line, so every clip
under test sits deliberately off-center.

Two things a clip must *not* restrict follow from the same care. A GL clear is
subject to the scissor test where a Vulkan load-op clear is not, so the GLES
backend disables the test across the clear; and a blit is subject to it too, so
the test is disabled before a multisample resolve rather than at each call site
that resolves. A clip left enabled across either would leave most of the target
holding whatever it held before.

**A texture a paint samples travels beside the batch, not inside it.** A batch
is a description a recorder produces without touching a device, so it cannot
name a backend texture handle. An image material carries a slot instead, and the
table those slots index is supplied at submission. A slot with no entry is an
error rather than a fallback — sampling whatever happened to be bound would draw
a plausible picture out of a previous frame.

The target is borrowed mutably and the table immutably, so a batch cannot sample
the target it draws into. That restriction is real rather than incidental, since
reading an attachment being written in the same pass needs machinery this does
not have, and having the compiler state it beats discovering it as a picture
that differs by driver.

**Every draw binds a texture, even a solid fill.** One shader serves every
material kind, so it declares the texture whatever the paint is, and a
descriptor a pipeline statically uses must be bound however unreachable the
branch reading it. The alternatives are a pipeline variant per material kind —
multiplying the cache to avoid one binding — or an unbound descriptor, which is
invalid. A draw that samples nothing binds a one-pixel white placeholder, owned
by the context rather than by a submission so a deferred submission can use it
without its descriptor pool outliving the fence.

**Tile modes live in the shader, not in samplers.** Address modes are a property
of the paint, so baking them into samplers would mean one sampler per
combination and a descriptor set per draw that used a different one. One sampler
stays fixed at clamp-to-edge and the shader does the wrapping. Both backends
must agree on that clamp: GL textures default to repeating, and the difference
is invisible until something samples an edge, where a linear filter blends with
the texel from the opposite side.

The image material was expected to break the 128-byte push-constant budget and
does not. Its mapping reuses the same origin-plus-matrix pair a radial gradient
needs — clip space is anisotropic on a non-square target, so both must map back
before measuring — and the texture is a binding rather than data. What would
break the budget is a material wanting a gradient's stops and an image's mapping
at once; nothing does yet, and that is when a uniform buffer becomes the answer.

**Upload is the exact inverse of readback**, in the same tightly packed
top-row-first layout, so a round trip through the pair is the identity on both
backends. That is what makes it checkable without a decoder. Decoding images
stays out of scope; getting already decoded pixels onto the device does not,
since without it an image paint could sample nothing but what the renderer had
already drawn.

**A recording is a list of passes, not one batch.** A layer is drawn into a
target of its own and composited back, which cannot happen in the pass that
samples it — reading an attachment being written needs machinery this does not
have. The separation is what makes group opacity mean "make this subtree, then
fade it" rather than "fade each shape in it": two overlapping half-transparent
shapes drawn directly show where they cross, and the same pair inside a
half-transparent layer does not.

Passes are stored in the order they finish and executed in that order, which is
already correct rather than something to sort: a layer is filed when it is
restored, necessarily before the draw that composites it. Each pass carries its
own slot table, since a layer occupies a slot alongside the caller's images and
the two number independently — invisible in a recording that uses no layers,
where they map one to one.

A layer clears to transparent rather than to the frame's background, because it
is composited over what is already there and anywhere it drew nothing must
contribute nothing. It starts unclipped: the draw that composites it is subject
to the clip that was in force when it opened, so content the clip excludes is
discarded once instead of prevented from being drawn. That costs some work
inside the layer and saves rebuilding a stencil clip in a second target.

**A layer's target is the size of its bounds, where the caller states them.**
`save_layer` with no bounds allocates a target the size of the frame, which is
the only safe answer when nothing is known about what the layer covers: a layer
is opened before its contents are recorded, so the recorder cannot measure them
without deferring the allocation. `save_layer_bounds` takes the caller's promise
instead, and a layer over a tenth of the frame then costs a tenth of the memory
and a tenth of the fill. The promise is enforced rather than trusted — content
outside the region is clipped by the target's own edges, so understating the
bounds shows as drawing cut off rather than as reading past an allocation.

The bounds are stated in user space and taken to device pixels through the
transform in force, rounded outward to whole pixels so a fractional edge never
loses coverage, and narrowed to the enclosing target. Whole pixels because the
composite samples the layer one texel to one pixel, which only stays exact on an
integer offset. An empty region falls back to a full-size layer, since a
smaller target would be a guess and guessing wrong loses drawing. Under a
rotation the region becomes a quadrilateral and the target is the box around
it, which covers more than was promised — the safe direction, a target being an
allocation rather than a clip the caller can observe. That is the opposite of
what `clip_rect` does with the same box, and for the same reason: there the box
would admit pixels the caller asked to remove.

Placing a target inside the frame means two mappings have to agree about where
it is: geometry is tessellated in the frame's device pixels and projected onto
the target's clip space, and a paint states its geometry in user space and
carries the inverse mapping back. They are derived from one description of the
target for that reason. The guarantee is that bounds are invisible, and it is
stated as a corpus mutation: every scene with a bounded layer is rendered again
with its bounds stripped, and the two must match exactly. Stripping the
original rather than writing the scene out twice is what keeps the pair from
drifting; requiring a match is the mirror of the clip mutation next to it,
which requires a difference. It runs on every backend rather than on whichever
comes first, because an offset that a full-size layer hides is exactly the kind
of thing two backends could disagree about.

**A recording is submitted, not just a batch.** A frame with layers is several
passes, and for a while the presentation paths took a batch — so such a frame
could be rendered offscreen and never displayed. The layer passes are
prerequisites of the root rather than part of the frame's pacing: they go
through the waiting submission, which is what orders them before the root
without a semaphore each. Only the root goes through the synchronized path,
which is the right split, because the acquire and present semaphores are about
the pass that touches the image being displayed and that is exactly the root.

Both presentation paths take a recording: the swapchain composites into the
image it acquired, and the scanout ring into the buffer the display will read.
The layer targets stay with the frame slot in each, released when it comes free
— after the fence has signalled, or after the kernel has flipped a commit it
gated on that fence, so in both cases the submission that sampled them has
finished.

The root samples the layer targets, so a deferred submission had to be able to
sample at all — it could not, and the descriptor sets it needs now travel with
the fence alongside the framebuffer, for the same reason. The targets
themselves stay with the caller: a fence is handed to a page flip and so has to
remain sendable, while a texture tracks its own image layout. Whoever owns the
fence releases them when it retires.

Pooling those targets was assumed to be worth doing and is not. Creating and
destroying a 512×512 target measures 0.6 microseconds against 53 for the
submission that draws into it, so a pool would save under two percent of what a
pass costs and would owe an invalidation rule in exchange. Bounding the layer
is the optimization that pays: on a four-layer frame it cut the time by 37%,
because it attacks the fill and the clear rather than the allocation.

A layer left open at `finish` is composited rather than discarded. An unbalanced
`save_layer` is a caller mistake either way, and dropping everything drawn since
it looks like a rendering fault rather than like the missing `restore` it is.

**A sampled texel is premultiplied and stays that way.** Every texture holds
premultiplied color, whether it was uploaded or rendered into, so an image paint
scales the whole texel by its alpha rather than treating the sample as straight
alpha and premultiplying afterwards. Doing the latter applies alpha twice. It is
invisible for an opaque image — the two conventions agree there — which is why
the first thing to expose it was a layer nested inside another layer, where a
half inside a half came out an eighth.

**Every vertex carries texture coordinates, including the ones that ignore
them.** Most geometry here locates itself from the interpolated clip position: a
solid fill and a gradient both do, and so does an image paint, whose mapping is
an affine in the material. A glyph run cannot. A run is many quads reading
different parts of one atlas, and a material is per draw, so coordinates carried
in the paint would mean a draw per glyph — and text is the highest draw-count
content there is, which makes that the wrong place to spend.

The cost is eight bytes on every vertex. The alternative, a second vertex format
and a second pipeline for text, spends more in pipeline state and in the code
deciding which of two shapes a batch is in, to save memory on the geometry that
was already cheapest to store.

**A glyph atlas holds coverage, not color**, in a single-channel format. The
glyph material reads one channel and scales a solid with it, which is what
antialiased text is; an image paint replaces color instead. The two differ in
the material and in where the coordinates come from, and share the binding
machinery underneath. Storing the same byte four times over works — the shader
reads red either way — and costs four times the memory and four times the
bandwidth to sample it.

The transfer paths had four bytes per pixel written in as a literal, which stays
invisible until a format has one. The size a caller must supply, the size read
back, and the channel layout a transfer names are all properties of the format,
and getting the last of those wrong reads three texels past the end of every
row.

Rasterizing an outline needs a font parser, and font parsing is out of scope —
bring `swash` or `ttf-parser` and hand over the coverage. That boundary is worth
more than tidiness: it lets the atlas be exercised with no font anywhere in the
tree, against bitmaps whose every texel is known, rather than against whatever a
hinter produced.

Packing is by shelves — a glyph goes on the first shelf tall enough with room to
its right. That loses the space above a short glyph on a tall shelf, which a
skyline packer would recover. It is the right trade here because the input is a
stream of boxes of very similar height, so shelves fill densely in practice, and
because the packer runs once per glyph per size rather than per frame. Each
glyph is padded by a texel: a linear filter samples a neighborhood, so a glyph
flush against its neighbor bleeds that neighbor into its own edge.

**Room is made by compacting, not by freeing.** Shelves cannot release a glyph
in place — a hole in the middle of one is reusable only by a glyph of the same
height, and tracking holes is most of what makes a general packer expensive. So
a full atlas keeps what the current frame has asked for, discards the rest, and
repacks from scratch. That is heavier than freeing an entry and far easier to be
sure of, and its cost is bounded by how rarely it can happen: it runs only when
an insertion would otherwise fail, and it cannot run twice in a frame without
the second failing outright, since everything left after the first is something
that frame needs.

An atlas with nothing stale to discard grows instead, doubling until it reaches
the limit it was given — the device's maximum texture size, which the atlas has
no way to ask about and so is told. Compaction is tried first, because
discarding what nothing has asked for is far cheaper than doubling, and an
atlas that grew before compacting would keep memory it had stopped needing.

Growing rather than paging is a decision the vertex format makes for us.
Texture coordinates travel on the vertices precisely so a run of any length is
one draw; a run spanning two pages samples two textures and is two draws, which
spends the property the whole arrangement exists to provide. That is a
correction to what this document said before, which named a second page as the
answer.

At its limit, with every glyph in use, it reports itself full. Evicting one
about to be drawn would trade a clear error for a wrong picture.

A glyph counts as used when it is *inserted*, not when it is looked up. That is
the usage the atlas is built around — a caller offers every glyph of every run
each frame and pays only for the new ones — and marking on lookup would need a
unique borrow at the point where a run is being recorded from a shared one. An
atlas never told that a frame ended keeps everything, which is correct rather
than a leak: a caller that has never said a frame ended has never said any glyph
stopped mattering.

Compaction moves every surviving glyph, so a repacked atlas reports itself dirty
and the coordinates a run reads are the ones current when it was recorded.

The atlas holds its texels rather than a device texture, and says when it is
dirty. Uploading is the caller's, because only they know which device the
texture lives on and when in the frame it is safe to write.

**A presentation target takes a surface; it does not make one.** Creating
windows is no more this project's business than mode setting is. An application
already has a window system connection and a window, and turning those into a
`VkSurfaceKHR` is one call, where owning that relationship would mean owning a
windowing library and its platform matrix.

That boundary is also what makes the swapchain path testable.
`VK_EXT_headless_surface` produces a surface with no window behind it, so
capability queries, format negotiation, present mode selection, acquisition,
presentation and recreation all run — and all are checked — on a machine with no
display, no compositor, and no window system library linked. What a real window
adds and this cannot reach is an extent the surface dictates and a surface going
out of date underneath a frame; both are handled, and both are unverified here.

**A GLES window frame is rendered offscreen and blitted in, flipped.** It could
go straight into the window's own framebuffer, and it would be upside down.
Everything this renderer draws puts the image's top row at a framebuffer's row
zero — the shader translator negates Y for GLSL, which is what makes readback
need no flip and the two backends agree pixel for pixel — while a window system
reads row zero as the bottom of what it shows.

Blitting with the source's rows exchanged puts that flip in exactly one place,
at the moment the image stops being something this renderer reads and becomes
something a window system does. The alternative is a second orientation
convention threaded through the projection, the scissor, the stencil and the
readback, each behaving differently depending on where the frame is going. The
cost is a full-screen blit per frame, which is what an offscreen-then-present
design costs anywhere and what a multisampled frame already pays for its
resolve.

An EGL pbuffer is the counterpart of a headless Vulkan surface: creation, being
made current, the blit and the swap all run with no display. What no test can
observe is what a window system would actually show, so the property checked is
the one that decides it — a presented frame is the vertical mirror of the frame
as rendered — against scenes deliberately not symmetric in the axis under test.

A swapchain is built at a size the caller asks for. There is no useful default:
a surface that defers its size clamps an unstated one up from zero to its
minimum, which produces a swapchain one pixel across that behaves correctly in
every other respect — it acquires, presents, cycles images and rebuilds, and
only a test that reads pixels back notices.

Surface formats are chosen non-sRGB. Color is linear inside the renderer and the
attachment format applies the transfer function, so an sRGB surface format would
apply it to values that already carry it. Present mode falls back rather than
failing: FIFO is required of every implementation and mailbox is a latency
preference, not a correctness requirement.

Swapchain images are wrapped, not owned. The presentation engine allocates them
and destroys them with the swapchain, so the texture type carries a third memory
kind that frees nothing — a distinction the type system keeps rather than a
comment.

**Presentation is ordered on the device, not by blocking a thread.**
Acquisition signals a semaphore, the render waits on it and signals another, and
presentation waits on that. Nothing blocks between acquiring an image and
drawing into it, or between drawing and presenting.

One wait remains and it is the one that should: before reusing a frame slot's
semaphores, the frame that last held them has to have finished. That is what
bounds how far ahead of the display the renderer may run, and it is reported —
a count climbing to one per frame means the GPU has become the limit, which is
worth seeing without a profiler.

The render-finished semaphore is per *image*, not per frame slot. Presentation
waits on it and the engine decides when it is done with an image, so one reused
while a present still refers to it is a wait on a payload already consumed.

The present layout is the render pass's final layout rather than a transition
afterwards, and that is correctness rather than economy: a separate transition
is a separate submission, and nothing orders it after a render that has not been
waited for.

Because those semaphores cannot travel through the backend-agnostic submission —
which waits for completion, putting a stall exactly where they exist to remove
one — a swapchain frame is drawn through the target's own `submit`, as a scanout
frame already is. Presenting a frame drawn any other way is refused rather than
half-synchronized: acquisition's semaphore would be signalled and never waited
on, and presentation would wait on one nothing signalled, which hangs rather
than looking wrong.

**Transient resources belong to the submission that reads them, not to the
context.** A single list of retained buffers is correct only while at most one
frame is in flight; with two, retiring the older fence frees the newer frame's
geometry out from under the GPU. Holding them on the fence makes "still in use"
a property of the submission it actually describes.

That defect lived under a test suite that already ran the deferred path with
validation on, and survived because every test in it retired a fence before
submitting again — so only one submission was ever outstanding, which is the one
arrangement in which the bug cannot happen. Coverage of a path is not coverage
of the states that path can be in, and a frame loop running two frames deep is a
state worth naming. **Anything that touches a device runs under the validation
layer**, and the presentation suites do now as well: they drive the deferred
submissions and the fences that gate them, which is exactly where a resource
freed early shows up, and nowhere that a picture would.

**Multisampling is a pass property, not a target property.** A pass renders
into a transient multisample buffer and resolves into the target, so the target
stays single-sampled and directly readable. Each backend realizes that
differently — a resolve attachment on one, a blit resolve on the other — and
both produce identical pixels, which is the kind of agreement the pass-level
description is meant to allow. The multisample attachment is never
stored — its contents are consumed by the resolve — which on a tiler keeps it in
tile memory rather than writing it out. A multisampled pass must clear. Seeding the multisample buffer from a target's
existing contents has no reverse-resolve to do it with on one backend and no
legal single-to-multisample blit on the other, so this is a property of the
technique rather than of a backend, and preserving is refused rather than
silently discarding what was there.

**Draws within a batch keep submission order.** Sorting by pipeline would cut
bindings further, but 2D drawing is painter's-algorithm ordered and reordering
two overlapping draws changes which ends up on top. Knowing when a reorder is
safe needs overlap analysis or a depth buffer, and belongs to the layer that
knows what the draws represent.

**Thread-safe resource creation is not yet met.** These methods take `&mut
self`, so a context cannot create resources from several threads at once. The
intended design is creation behind `&self`, which needs interior mutability
around the allocator; that is deferred rather than decided against, because
nothing creates resources off the recording thread yet and the synchronization
would be shaped around a caller that does not exist.

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
entire golden and conformance apparatus runs on it, which makes it the most
exercised target and the one every other target's output is compared against.
Presenting must change nothing about what was rendered — a target decides where
an image goes, not what it contains — and that equivalence is asserted over the
whole corpus.

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
advertises what it can render to and export, queried from the device rather
than assumed. Without a real advertisement the only safe assumption is linear,
which works everywhere and wastes bandwidth everywhere.

An exportable image differs from an ordinary one in two ways, both fixed at
creation. It is created with an explicit modifier chosen from the negotiated
set, because an optimally-tiled image has a layout only its own GPU
understands. And its memory is a dedicated allocation rather than a
suballocation, because a dma-buf hands over a whole allocation — an image
sharing one with other resources cannot be exported without exporting them
too, so that case is refused rather than over-shared. The intersection is chosen from,
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

The required surface is expressed as a trait (`ScanoutOutput`) rather than
consumed directly. That states exactly what drm-rs must provide, so the two
projects can be sequenced against each other rather than discovering a mismatch
at integration, and it makes the parts most worth testing runnable without a
display: ring accounting and fence plumbing are where this path goes wrong, and
neither needs real hardware to go wrong. What a stand-in display cannot check is
whether a real controller accepts the buffers, which is what the VKMS lane and
the board rack exist for.

**A slot can be held by two different things, and they need different waits.**
A buffer the display still owns frees when a flip completes; a buffer the GPU
still owns frees when its fence signals. Waiting for a display event in the
second case waits for something that is not coming, which stalls the loop until
it times out rather than failing outright.

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

- `Context` is intended to be `Send + Sync` with thread-safe resource
  creation. Not yet met: creation takes `&mut self` today, see the HAL section.
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

**Clip space follows the WGSL convention, with Y increasing upward** — not the
Y-down convention Vulkan's framebuffer uses natively. Translation to each
backend's native space happens during shader translation, which is what makes
one source produce matching output everywhere instead of vertically mirrored
output on some targets. Mapping user space, where Y typically runs downward,
onto clip space belongs to the renderer's transform stack. A test pins this,
because disabling the adjustment mirrors every shader on Vulkan while leaving
other backends untouched.

naga output for every shader and target is snapshotted in the repository and
diffed in CI, so a naga upgrade that changes codegen is a reviewed event rather
than a silent behavior change. Specialization maps from one set of declarations
in the WGSL source: spec constants on Vulkan, bounded build-time macro
permutations on GLES.

## Renderer internals

- **Geometry** (`impeller-geometry`): lyon for general fills and strokes;
  convexity detection for a fan-fill fast path; Wang's-formula adaptive Bezier
  flattening with transform-aware scale. Analytic coverage for rect, rrect,
  circle and ellipse — computed in the fragment shader instead of tessellating,
  which is where most of a real interface's draw calls land — is **not
  implemented**: there is no rounded rect or ellipse in the tree, and a circle
  is four cubics flattened like any other curve.

  Convexity is a correctness question, not only a speed one. A fan fill
  triangulates from one vertex and has no notion of a fill rule, so a polygon
  wrongly called convex is filled by a routine that cannot express what filling
  it means, and the path's fill rule is silently discarded. Agreement of turn
  directions does not settle it — a pentagram turns the same way at all five
  points and crosses itself five times — so the total turning is counted too: a
  simple closed polygon comes back to its start having turned once around, and
  a self-crossing one turns twice or more. Counted by quadrant advances rather
  than accumulated as an angle, which is exact, allocates nothing, and measured
  four times faster than the `atan2` per vertex the definition suggests. Both
  are in the tree and a test requires them to agree, so the cheap one has
  something to be checked against.
- **Entity layer** (`impeller-entity`): **not built.** The crate is two lines
  of module comment. The design is that an entity carries transform, blend,
  clip depth, contents, and geometry, with a `Contents` implementation per
  material and coverage computation for culling — and nothing needs it yet,
  because the canvas records into a batch directly and the layer would sit
  between two things that already fit. Building it before there is a caller
  would fix its shape around a guess.
- **Passes** (`impeller-renderer`): draws are accumulated into a batch and
  submitted as one pass, in submission order rather than sorted by pipeline
  (see above). Save layers become offscreen
  targets with a paint-composited restore, sized to the caller's bounds where
  given; path clipping is stencil-based, and bounded at 255 levels because that
  is what eight bits of stencil counts to and eight is what every device is
  required to offer. Deeper is refused rather than allowed to saturate, since a
  saturated count makes the innermost clip stop excluding anything — content
  the caller clipped away is drawn, and nothing about that reads as an error.
  The check lives beside the depth it reads rather than in a backend, because
  it was in one backend and not the other and the same recording was therefore
  an error on Vulkan and a wrong picture on GLES. Blur is **not implemented** — there is
  no blur anywhere in the tree, and a multi-pass separable one is the intended
  shape rather than something that exists.
- **Text** (`impeller-text`): shelf packing of caller-supplied coverage into
  one texture, with compaction and then doubling when it fills. Rasterization
  is out of scope and lives with the caller's font parser, which is what lets
  the atlas be tested against bitmaps whose contents are known exactly.
  **Not implemented**, and named because this list claimed them: subpixel
  quantization, SDF above a threshold size, COLR/CPAL color glyphs, and
  paging — a page boundary would split a glyph run into more than one draw,
  which is why growth was chosen over pages.
- **Color**: linear f32 internally, with sRGB conversion at the API boundary
  and at target write. Linear internally because blending and interpolation are
  operations on light: averaging two encoded bytes is not averaging the two
  colors, and a gradient built that way is visibly wrong in its middle. The
  conversion on the way out is the target format's, not a shader's — an sRGB
  format encodes on write, so nothing in the pipeline knows the difference and
  a caller picks it when creating the surface.

  The property that ties the two together is a round trip: a color authored
  through `Color::srgb` and drawn into an sRGB surface comes back as the byte
  that was authored. That holds exactly on both backends, and a test also
  requires the same drawing into a linear and an sRGB surface to differ by
  exactly the transfer — which is what says the pipeline carried light the
  whole way and only the final write encoded, rather than converting early or
  twice.
- **Materials**: a paint resolved for a backend — colour or gradient, already
  in clip space — packed into 112 bytes, inside the 128 of push constants every
  device is required to offer. Staying within the guaranteed minimum is
  deliberate: a part that provides only the minimum is exactly the embedded
  hardware this renderer targets, and a material that did not fit there would
  fall back to a uniform buffer on the devices least able to afford one. The
  limit is a compile-time assertion rather than a test.

**A gradient locates itself from an interpolated clip position, not from the
fragment coordinate builtin.** That builtin's origin differs between the two
graphics APIs, so using it would run gradients in opposite directions on each
backend. Clip space is normalized by shader translation and agrees everywhere,
and passing it as a varying also avoids a second vertex attribute that every
solid draw would otherwise pay for. Gradient endpoints travel through the same
transform the geometry does, so a gradient rotates and scales with its shape
rather than staying pinned to the screen.

## Crate layout

| Crate | Owns |
|---|---|
| `impeller` | Public facade; carries the feature flags |
| `impeller-core` | Public API: Canvas, Paint, Path, Recording, glyph runs |
| `impeller-entity` | Entity and Contents layer — **a stub; nothing is built** |
| `impeller-geometry` | Path types, tessellation, fast paths |
| `impeller-renderer` | Render pass encoding, generic over the HAL |
| `impeller-text` | Glyph atlas: packing, compaction, growth. Not rasterization |
| `impeller-hal` | Rendering HAL trait |
| `impeller-hal-vulkan` | Vulkan backend |
| `impeller-hal-gles` | GLES 3.0 backend |
| `impeller-present` | Presentation trait, shared types, negotiation |
| `impeller-present-vk` | Vulkan WSI swapchain target |
| `impeller-present-egl` | EGL window-surface target |
| `impeller-present-drm` | DRM/KMS scanout target |
| `impeller-shaders` | WGSL sources and build-time translation |
| `impeller-testkit` | Shared test harness |
| `xtask` | Capability reporting; device runs, golden management and CI reproduction planned |

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
to aarch64 and riscv64 boards and containerized CI builds trivial. `deny.toml`
describes this as a ban on `pkg-config`, `cmake` and `cc`, and describes it as
automated enforcement, which it was not: nothing invoked cargo-deny. It is now
enforced by a test that reads the workspace's dependency graph and fails on any
crate whose job is to compile, locate, or generate bindings to something that is
not Rust. That needs nothing installed, which matters for a rule whose whole
value is that it holds on a machine nobody has configured.

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

**One corpus, many executions.** A single corpus drives every execution there
is, so a new feature adds scenes once and the matrix multiplies coverage.
Every rendering bug fixed adds a regression-pin scene, and that set grows
monotonically.

What that means today, stated precisely because the aspiration and the state
are easy to confuse. The corpus is Rust: a couple of dozen `Scene` values built
in `impeller-testkit`. The intent is for scenes to be data — a versioned
serialized IR of canvas calls — so that a corpus can be shared with a board and
with upstream's assets, and nothing is serialized yet.

`impeller-testkit` provides one executor, which renders a scene offscreen, and
two comparators: per-channel tolerance with an outlier budget, and the
derivation below that chooses between exact and tolerant. The executions it
drives are the cross-backend comparison and the cross-device conformance run.

A scene becomes a recording by driving the canvas, not by building a batch.
That is what lets a scene say anything the public API can, and it is a
correction: the executor used to resolve gradient endpoints, convert clips to
scissors and step the stencil itself, which was a second implementation of the
canvas that drifted from the first as soon as the scene format grew — and which
could not express a layer at all, layers being the canvas's own idea. So layer
compositing went uncompared across backends for as long as that lasted, and a
separate harness had to be written to cover it before this was folded back.

A scene is therefore a tree rather than a list, since a layer contains things.
Nothing else about the format nested, and the flat constructor is unchanged, so
a scene with no groups reads exactly as it did. The derivations that ask what a
scene contains — its tolerance, the capabilities it needs — walk the tree
through one iterator rather than each learning its shape.

Not yet built, and named here rather than described in the present tense: WSI
and DRM executors, a perceptual comparator for cases where cross-driver float
variance is expected, CRC equality for scanout, and a report schema emitted
identically by CI and by a shell on a board.

Tolerance is derived from what a scene does rather than assigned per scene:
**exact where a value is transported, tolerant where it is computed per
fragment.** A solid fill copies a colour through the pipeline, and any
difference there is a defect. A gradient evaluates one, a blend converts an
intermediate to fixed point, and a multisample resolve averages — none of which
the specification requires to be bit-identical, since shader arithmetic is
permitted some error and compilers may fuse operations differently. Deriving it
means a new scene inherits the right rule instead of acquiring a hand-set
number, and a genuine divergence cannot be waved through by loosening one entry.

Where per-driver tables become necessary, tightening one is a normal change and
**loosening one requires a linked driver-bug issue**.

The levels below are the plan. The right-hand column says where each runs, and
**"CI" describes none of them: there is no CI**. What exists is `ci/smoke.sh`,
run by hand, which does the whole of L0, L1 and L3 and part of L4.

**Every context in the suite reports what the driver said about it.** On
Vulkan that is the validation layer; on GLES it is `GL_KHR_debug`, which every
3.2 implementation offers and which is the driver reporting on itself rather
than a layer to load. The two cover different ground — the layer tracks object
lifetimes and synchronization, and `KHR_debug` does not — but `KHR_debug` covers
what a state machine gets wrong, which is a call made with the wrong state
bound, and that is most of what this backend can get wrong. Messages are
requested synchronously so one arrives inside the call that caused it. Both are
read by a guard that asserts when the context drops, and both classify severity
the same way, so "no errors" means one thing across the two.

On Vulkan the layer is asked for **synchronization validation** as well, which
it does not enable by default. Core validation checks that each call is well
formed; this checks that one access is ordered against the next. That is the
distinction that matters to a renderer synchronizing explicitly, because a
missing barrier renders correctly on the device it was written on and wrongly
on the next one — there is no wrong picture to notice.

Turning it on reported one immediately. Every render pass here was created with
no subpass dependencies at all, so an attachment's layout transition — which
happens as part of beginning the pass — was ordered against nothing outside it.
Waiting on the acquire semaphore does not cover it: a wait whose destination
stage is color-attachment output orders the draws, while the transition is free
to run before the wait completes. The layer reported it as a write-after-read
against `vkAcquireNextImageKHR`: the pass could transition a swapchain image
while the presentation engine was still reading it. Both directions are now
declared, over every stage that touches an attachment rather than only the
color one, since a stencil attachment is transitioned and then cleared by its
load operation and those are two writes needing the same ordering.

Best-practices checking is a third thing the layer offers, and is run by hand
rather than left on. It is what found the image barriers: every layout
transition named every pipeline stage and every kind of memory access, which is
always correct and is a full pipeline drain and cache flush each time — on the
tiled, bandwidth-limited parts this targets, not a small waste. A layout says
what the image was being used for, so the scopes are derived from it, with
anything unrecognized falling back to naming everything. That fallback is the
safe direction: a scope too wide costs speed, and one too narrow is a missing
barrier that renders correctly here and wrongly elsewhere. It is not left on
because much of its advice is vendor-specific, so a suite failing on it would
fail on somebody else's GPU for things that are not defects.

**The Vulkan half of that runs with the validation layer on, and
asserts what it reported.** Capturing the messenger is what makes that
assertable; a guard that checks the log when a context drops is what makes it
asserted, which is not the same thing. A test that turns the layer on and never
reads what it said is indistinguishable from one that left it off, and a check
written out in each test is one some test will be missing.

That was not hypothetical: the layer was on for the Vulkan backend's own tests
and off for the whole shared harness — the scene corpus, the cross-backend
comparison, the conformance run, clipping, blending, sampling — which is the
largest body of rendering here. Removing a layout transition that a sampled
texture needs leaves every one of those tests passing on this driver, with
correct pixels, and fails six of them once the layer is read. The class of bug
it catches was invisible to the part of the suite that renders the most, and
costs about eight percent of that suite's run time to see.

**A skipped test passes, and `cargo test` hides which.** A test that finds it
cannot run — no device, no second backend, a capability the hardware lacks —
prints why and returns, because the alternative is a suite that cannot be run
on the machines the suite exists to cover. The harness then captures that
output, since the test passed, and the reason is discarded. So a green run says
nothing about how much of it ran, and the two ways it can be green look
identical from outside.

That is not hypothetical here. Three tests comparing the two backends had been
skipping since they were written, because the crate they live in compiles one
backend by default and asking for the other simply failed. A whole file of DRM
tests skipped because modesetting master is exclusive per device and the
harness runs one file's tests on several threads, so all but the first were
refused the card. Both were found by looking, not by a failure.

`cargo xtask verify` runs the suite with output uncaptured and reports the skip
census alongside the counts. It does not treat a skip as a failure, because
some are correct — a device without the advanced-blend extension genuinely
cannot render those scenes, and the corpus reports that as coverage it did not
get rather than as a pass. The point is that the number is visible and has to
be looked at, since the incorrect ones look exactly the same from there.

| Level | What | Where | State |
|---|---|---|---|
| L0 | Unit: math, path ops, atlas packing, negotiation logic | Every merge, no GPU | runs |
| L1 | Property: tessellation invariants, Bezier tolerance, stroke under transform | Every merge, no GPU | runs |
| L2 | Golden: corpus to offscreen render, image compare | Every merge on software GPU | none — comparison is against another implementation rather than a stored image, deliberately, and no golden apparatus exists |
| L3 | Conformance: same corpus, cross-backend and cross-presentation diffs | Every merge (software) | runs, cross-backend and cross-device; cross-presentation only for the offscreen target |
| L4 | Presentation: resize storms, flip pacing, fence ordering, hotplug | VKMS and headless WSI in CI | partial — headless WSI runs on both backends, fence ordering is checked under the validation layer; no VKMS, no resize storms, no hotplug |
| L5 | Stress and soak: atlas thrash, layer-depth bombs, leak detection | Nightly and weekly, hardware | none |
| L6 | Performance: micro and full-frame benches with regression gating | Nightly, quiet runners | none — there is no benchmark in the tree |
| L7 | Fuzz: path data, scene descriptions, dma-buf negotiation | Continuous background | none |

### VKMS would give the DRM path merge-blocking coverage

**Not set up.** Nothing in this section runs; it records why the lane is worth
building. `cargo xtask drm` reports whether a given machine could host it.

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

`impeller-capi` builds `libimpeller`, intended as an ABI-compatible
implementation of upstream Impeller's C API
(`impeller/toolkit/interop/impeller.h`). The goal is binary compatibility: a
consumer linking the upstream C API could link this instead without
recompiling. The shared BSD 3-Clause license is what makes vendoring the header
and upstream's test assets clean.

**Almost none of it exists.** Version negotiation is implemented and
`ImpellerGetVersion` is the only exported symbol. The rest waits on the header
being vendored, and not merely for tidiness: guessing an enum value or a struct
layout produces a library that links and then corrupts memory, which is a worse
outcome than one that does not link.

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

Two checks are intended and neither exists yet. Symbol-level parity is to be
checked mechanically rather than maintained by hand, by diffing exported
symbols against a pinned copy of the upstream header, so that an upstream
addition surfaces as a failure naming the missing entry points; that waits on
the header being vendored. Semantic parity — blend mode values, fill rules,
color handling, stroke geometry — is to be checked by running upstream's C API
samples against this library and comparing output through the usual golden
comparators. Semantics, not symbols, are the hard half.

What exists is narrower, and worth not mistaking for either: a test that the
exported symbols are exactly the ones written down beside it. That catches an
accidental export and keeps the status claim from drifting away from the code.
It says nothing about whether the surface matches upstream's.

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
