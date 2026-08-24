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

This is the design. Boxes marked *planned* are not built, and the section on
what exists says so again in more detail — but a diagram is what a reader looks
at first, so it says which parts are drawings of intent rather than of code.

```
┌───────────────────────────────────────────────────────────────┐
│  Public API (impeller-core)                                   │
│  Canvas, Paint, Path, Layer, Recording                        │
├───────────────────────────────────────────────────────────────┤
│  Entity layer (impeller-entity)   coverage only; not routed   │
├───────────────────────────────────────────────────────────────┤
│  Renderer (impeller-renderer)  — generic over Hal             │
│  Tessellation into clip space, batch assembly                 │
│  (planned: pass sorting, pipeline cache, frame allocators)    │
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
disagree about what a mode means. All of them assume premultiplied color, and
alpha uses the same factors as color: with premultiplied color the alpha
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

**A difference clip narrows by the complement, which is two contours and an
even-odd rule.** `dart:ui` spells it `clipRect` with `ClipOp.difference`, and it
is the only clip that operation applies to there -- `clipPath` and `clipRRect`
intersect and take no operation. It can never be a scissor whatever the
transform, the complement of a rectangle not being one, so it narrows through
the stencil like an arbitrary shape: the target with the rectangle taken out of
it, which is the target's contour and the rectangle's filled by even-odd. A ring
is built the same way.

Both contours are in device coordinates and filled through the identity rather
than in user space through the transform, and the reason is the target: it is a
device rectangle, and expressing it in user space would mean inverting a
transform that may not be invertible. The rectangle's own corners go through the
transform instead, so a rotation removes the quadrilateral rather than a box
around one -- which is a third again more area, and is what the test measures.

The tracked clip bounds are deliberately not narrowed. Removing a rectangle from
the middle of a region leaves its bounding box where it was, and removing one at
the edge leaves a box that is too large -- the safe direction, since those bounds
decide how much a caller draws and how large a layer is allocated.

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
at once, and that is when a uniform buffer becomes the answer.

A full budget is not the same as a budget spent well, and the conical gradient
is the case that showed the difference. It needed one float more than a
gradient had, and the obvious reading — the budget is exactly full, so this
waits for the uniform buffer — was wrong. A whole float was carrying a boolean
saying the colors had been baked into a texture, and a material whose colors
are in a texture has no stop count to report, so the count now carries both:
zero stops means read the ramp. Nothing about the mechanism had to change. The
rule that came out of it is to look for a field paying for less than its width
before concluding that a limit has been reached, and to be suspicious of any
plan that begins by moving everything somewhere larger.

**Materials travel in a uniform buffer, not in push constants.** The paragraphs
above are kept as written because the reasoning in them was sound and the
conclusion still arrived: the case that broke the budget was a material sitting
on top of another one. A color filter is exactly that — it applies to whatever
material is already there, including a four-stop gradient that uses every float
— and a color matrix alone is twenty floats. No rearrangement holds both.

The reason to hesitate was the hardware. Push constants are the cheap path for
small per-draw data on tile-based parts, and the 128-byte guarantee exists
because parts offering only the guarantee are exactly the ones this targets. So
the question is not what is elegant but what that hardware does well, and the
answer came from Impeller itself: it places per-draw uniform data in a
per-frame host buffer, aligned to the backend's minimum uniform offset
(`HostBuffer::EmplaceUniform`), on the same class of parts. That is a uniform
buffer with a dynamic offset per draw, which is what this does now.

One buffer per submission holds every draw's material end to end, each padded
to the device's minimum offset alignment, with one descriptor set rebound at a
different offset per draw. On Vulkan it is a second descriptor set rather than
another binding in the texture set, because that set includes a placeholder
owned by the context and outliving any one submission. On GLES it is a uniform
block, which is what this document already specified for that backend and what
the implementation had not yet done — so the two backends now agree on
mechanism as well as on layout.

The layout is `std140`, stated rather than assumed. A block with no qualifier
is `shared`, whose member offsets the implementation chooses and a caller is
expected to query; both backends write the bytes themselves, so they need the
layout every implementation agrees on. The translator writes the qualifier only
alongside an explicit binding, and GLSL ES 3.00 has no `layout(binding = )` for
a uniform block — that arrived in 3.10, above this project's floor — so the
build step adds it, asserts it found exactly what it expected, and a test reads
the generated source back to confirm.

Changing the mechanism is not a licence to change the size, and the size is
stated once -- below, where the number is under test -- rather than restated
here where it would go stale. It did go stale here once, which is why.

The premise behind a widening is worth checking before acting on it, because
the one just before this was wrong. A conical gradient looked like it needed
the same move and did not: a whole float was carrying a boolean, and a material
whose colors come from a texture has no stop count to report, so the two folded
into one number and the material never grew. A color filter genuinely does need
the room -- it applies on top of whatever material is already there, so its
cost is additive to a gradient that already uses every float. Ask whether a
field is paying for its width before concluding a limit has been reached. That
is not an argument against changing a mechanism when it does have to change.

**A picture is its passes appended and its root sampled.** `draw_recording` is
`drawPicture`, and building it needed less than the note below predicted. That
note expected the work to be carrying clip-space positions and each material's
own geometry through the composite transform. None of that is done, because the
pass model already answered it: a layer is a pass another pass samples, so a
finished recording is its passes taken as they are and its root sampled by the
canvas drawing it. Nothing is re-recorded and no vertex is touched.

What is left is index arithmetic, and it is the whole of what can go wrong. A
picture's layers name passes by position, and its baked gradients name ramps by
position, in lists the receiving recording is appending to -- so both shift by
however much is already there. Wrong, and a picture's layer samples the host's:
a plausible picture of something nobody drew, which is the failure mode this
document keeps returning to.

Images are the exception and are not renumbered. A caller's image index means
the same thing in both recordings because both are submitted against one table,
and a caller composing pictures that disagree about what image three is has to
renumber before recording -- which is a thing they can see and this call cannot.

**A recording is tessellated geometry, not a command list.** Worth stating
because the name suggests otherwise and because it decides what nesting one
inside another could mean. By the time a draw reaches a batch its path has been
flattened -- at a tolerance taken from the scale of the transform then in force
-- and its vertices and its material are both in clip space. Nothing upstream
of that survives.

Two consequences. Replaying a recording under a different transform is possible
in principle, since both the positions and each material's geometry could be
carried through the composite -- which holds for a transform carrying
perspective as readily as for an affine, both being matrices the pair go
through together. But the flattening cannot be undone: magnified, a curve shows
the polygon it was flattened to. Perspective sharpens that rather than
softening it, since a recording replayed toward the viewer is magnified
unevenly and shows its polygon at the near end first. And a
recording is not a cache of drawing commands, so it cannot be re-rendered at a
new resolution without being recorded again. That is the trade for having no
retained state anywhere below the canvas, and it is the right one here -- but
it is the reason `drawPicture` would be a convenience rather than the reuse it
is in a command-list renderer.

**Every vertex writes a depth of zero, and that is what handles the horizon.**
A transform carrying perspective can send part of a shape past the vanishing
line, where the divisor passes through zero and the geometry does not collapse
but inverts. The usual answer is to clip the tessellated triangles against that
plane before they are drawn, which is real work and is not done here.

It is not needed, because the rasterizer already does it. Both APIs test a
primitive against the near plane as part of their clip volume -- Vulkan against
`0 <= z <= w`, GL against `-w <= z <= w` -- and with `z` pinned to zero each of
those reduces to `w >= 0`. So a primitive behind the vanishing line is
discarded and one straddling it is cut there, with the new vertices
interpolated by the hardware. What this costs is emitting a real `w` rather
than the constant one, which is the change anyway: a point divided by a
quantity that varies across a triangle has no two-component form to arrive in.

The consequence worth naming is that a decision which was arithmetic is now a
property of the pipeline. `invert_to_local` substitutes the identity for a
placement that cannot be inverted, and that was safe because the geometry went
through the same matrix and so had no area -- an argument about the numbers,
true unconditionally. Under a homography it fails: a matrix can be non-singular
and still send part of a shape past the horizon, where geometry blows up toward
infinity rather than collapsing toward nothing, turning "draws nothing" into
"draws the entire frame". What restores it is the clip above, which removes
every fragment whose divisor has the wrong sign before any of them consults the
mapping. That holds only while `z` stays zero. Emit a depth from somewhere and
the invariant goes with it, silently.

**Sampling quality lives in the shader too, for the same reason tile modes do.**
A sampler built with a filter would mean one sampler per combination of filter
and address mode, and a descriptor set per draw that used a different one. A
linear read taken exactly at a texel's center has all its weight on that texel,
so nearest sampling is the coordinate snapped to the nearest center before the
read — one multiply, floor and divide, and no bindings at all. The texture's
size comes from the shader rather than from the material, because the recorder
that built the material has never seen the texture.

The bicubic that `FilterQuality.high` means goes in the same place, and there it
is not a saving but the only place it could go: no sampler reconstructs a cubic,
so the sixteen reads and their weights are arithmetic in the fragment either
way. The curve is Mitchell-Netravali with `B` and `C` both a third, which is
what Skia's high quality has always been and therefore what a caller porting
from Flutter is expecting -- sharper than a pure B-spline, gentler than
Catmull-Rom. Its weights are a partition of unity, so nothing is normalized
afterward and a flat image comes back exactly flat, and they go slightly
negative between one and two texels out, which is the sharpening rather than a
fault in it. That overshoot is also why the result is clamped back into
premultiplied form: color and alpha ring by different amounts wherever they
step differently, and a channel above its own alpha is not a color.

**A batch merges adjacent draws that differ in nothing.** Two draws with the
same material, filter, blend, clip and stencil are one draw over a longer index
range: their indices were appended to the same buffer, so extending the first
range covers both, and the triangles rasterize in the same order either way.
That last part is what makes it safe under painter's-algorithm ordering, where
two overlapping shapes must not trade places.

Adjacent only. Sorting draws to create more of these is a different decision
with a different safety argument -- it needs overlap analysis or a depth buffer
-- and this one needs none, because the sequence is untouched. The check that
merging changed nothing is that rendering each draw as its own submission gives
the same pixels, which is a path that never merges anything.

**A gradient with more stops than the material carries is tabulated rather than
truncated.** Four fit, which is almost every real gradient and costs no texture;
past that the recorder evaluates the ramp into a small image and the shader
reads the color at the parameter instead of computing it. That is the case above
turned around rather than met: such a material wants a gradient's *mapping* and
a binding, and no longer wants its stops at all, so the budget is not the
constraint. The ramp is stored as linear half-floats and holds straight
rather than premultiplied color. Both paths therefore hand the same shape of
value to the same premultiply at the end, which is what makes the choice between
them invisible.

It was stored through an sRGB format, so that eight bits would be spaced the way
the eye reads them — linear eight-bit color bands in the darks. That argument
was right and is *answered* rather than overridden: half has no fixed quantum,
its precision being relative at about eleven bits of mantissa at every
magnitude, so there are no longer eight bits to spend well and the perceptual
spacing was buying what the format now gives everywhere. The reason for straight
storage changed with it — a transfer function not commuting with multiplying by
alpha no longer applies to a linear table — and the reason that survives is that
`gradient_color` returns the same shape from both arms, so a premultiplied table
would fork the two paths at the point the design exists to converge them.

What the old format also could not do was hold a component the sRGB primaries
cannot describe, and the invariant above quietly depended on it not having to.
Clamping into eight bits meant that a gradient of five stops and the same
gradient stated in four disagreed by twenty-four levels once a color filter
brought the difference back inside the range a target could show — the sort of
defect that hides because both halves of it look like rounding.

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

**A backdrop filter cuts the pass rather than reading it.** Frosted glass asks
for the one thing the rule above forbids: a layer whose starting content is the
target it is about to draw into. So the pass stops. Everything drawn into that
target so far becomes a pass of its own, what follows begins by drawing that
pass back in, and the layer is seeded with a filtered copy of it. Three extra
passes and one full-target copy per filter, which is the honest price of not
having the machinery to sample an attachment being written — and it is paid only
by a layer that asks.

That price is why the bounds of such a layer stop being an optimization. For
every other layer they say only where the content is and the picture is the same
without them; here they are the region filtered, so a frosted panel states its
bounds and the same layer without them blurs the whole frame. Both are
meaningful and they are different pictures, which is a distinction the API
documents rather than resolves.

The filtered copy is seeded with `Src` and the layer's own content composites
over it. A caller who replaces instead of blending erases the backdrop and gets
a layer that renders identically with the filter on and off — which is how the
first corpus scene for this was written, and what it now carries a comment
about.

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

**A morphological filter splits across passes rather than sampling sparsely.**
Dilation and erosion are separable in the same way a Gaussian is -- a
rectangular structuring element is the product of two intervals, so one pass
per axis gives the square of taps -- and the two filters share the machinery
that makes a pass over a finished layer. They part company at the tap budget.
A shader loop has to be bounded, and the blur handles a sigma past that budget
by spreading its taps further apart: a missed sample costs a little smoothness,
and the result is still a blur.

That trade does not exist here. The output is a maximum, so a sample the loop
skips is not a small error in a weighting, it is a scallop in the edge, and the
result is not a dilation by any radius. What does exist is a property the blur
lacks: morphology is *decomposable*. Dilating by `a` and then by `b` dilates by
`a + b` exactly, because flat structuring elements add under the Minkowski sum,
and the same holds for erosion. So a radius past one pass becomes more passes
and the answer stays exact. The radius is also rounded to whole texels on the
host, in the one place that both the shader and the layer's bounds read it
from: a structuring element is a set of sample positions and there is no half
of one, and a radius rounded in one place and floored in the other would grow
the picture by a pixel more than the target had room for.

The two filters also disagree with the blur about what lies outside the image.
The blur clamps to the edge, because a weighted average that read transparent
black from beyond the bound would darken every border pixel. Morphology reads
nothing, which is transparent black. For a dilation the two are
indistinguishable -- the largest of a value and transparent black is the value
-- and for an erosion the choice is the whole behavior: clamped to the edge, a
shape sitting against its layer's bound would never be eaten into from that
side, because every sample reaching past it would come back as more of the
shape. Only a dilation widens the layer's bounds, for the same reason: an
erosion never puts anything where there was nothing.

**A filter's blend belongs to the composite, not to the draw inside the
layer.** Everything that acts on a finished image -- an image filter, a mask
blur, and now a color filter over a caller's own program -- draws into a layer
and composites it back. The paint's blend mode has to ride that composite. Left
on the draw inside, it runs against the layer's own transparent black, so a mode
that reads its destination finds nothing there and yields the source unchanged,
which is then composited over the frame it was meant to combine with. `Plus`
over a cyan ground gave red where it should give white, in all three paths,
since each was written.

Only the outermost composite carries it. Peeling a chain of filters opens a
layer per link, and a blend carried down would apply once per link rather than
once, so the paint handed inward is neutralised to `SrcOver`. The mask blur's
own style blends are a different thing and stay where they are: those combine
the shape with its blur *within* the layer, which is exactly where a
destination-reading mode is supposed to look.

**A color filter over a runtime effect acts on the image, because there is
nowhere else for it to act.** Every other material is evaluated by this
renderer's own fragment shader, where a filter is four multiply-adds at the end
of it. A runtime effect is a whole pipeline: the caller's program is the
fragment shader, and nothing can be appended to it. So the filter was accepted
and silently did nothing -- no error, no effect, and no way for a caller to tell
which. It now draws the program into a layer and filters the composite, which is
what the filter meant anyway and costs what every other image-acting filter
costs. `Layer` grew a color filter for it, which `dart:ui` has independently:
`saveLayer` takes a paint, and that paint's `colorFilter` applies to the group.

**A glyph run is the third door, and it was open too.** A run does not pass
through `draw_path` any more than a mesh does, and it too accepted an image
filter and drew without one. It now routes through a layer like the other two,
taking its bounds from the run's own boxes since it has neither a path nor
vertices to take them from. Text is the highest-draw-count content there is,
which also makes it the place a dropped filter is least likely to be recognized
as a dropped filter rather than as bad text.

Its mask blur is built rather than refused, unlike the mesh's, and the
difference is not arbitrary. A mesh carries a color per vertex, so blurring its
coverage and blurring its result are different pictures and the refusal is
permanent. A run is coverage times one solid color, which is exactly the case
where the two agree -- and a text shadow is a mask blur over a run, so it is the
common case rather than a corner.

Building it took naming what a mask blur is blurring instead of passing it a
path. Every style draws its content two or three times -- into a blurred layer,
and again at full sharpness to combine with it -- so the content is a small enum
rather than a closure, a closure taking `&mut Canvas` not being callable twice
while the canvas is borrowed. Two variants is the whole set: the operation needs
coverage times one solid color, which is what rules a mesh and an image out.

**A mesh takes the same filter routing a shape does, because it does not pass
through the same door.** `draw_path` is where a paint's image filter and mask
blur are noticed, and a mesh never goes near it -- so both were accepted on a
mesh and silently dropped, and an atlas inherited that, being a mesh by the time
it arrives. The image filter now routes through a layer exactly as it does for a
shape, taking its bounds from the vertices since there is no path to take them
from. The mask blur is refused instead of implemented, which is the same answer
`draw_masked` gives a gradient and for the same reason: blurring coverage and
then filling is the same picture as blurring the result only where the fill does
not vary, and a mesh carries a color per vertex.

The two routes are written out separately rather than shared behind a closure.
They differ in exactly two places -- where the bounds come from, and which draw
call the remainder of the chain is handed back to -- and everything else about
them is the same, which is worth being able to read side by side.

**A chain of image filters is a stack of layers, peeled one at a time.**
`ImageFilter::Compose` holds two filters, so a paint can carry a chain of any
depth. Building the whole stack at once would mean walking the chain in the
recorder and opening every layer before drawing anything; instead the draw peels
the outermost filter, opens its layer, and hands the remainder back to the same
entry point, which lands here again if anything is left. The single-filter case
is then exactly what it was, with the remainder empty.

What that leaves to get right is how wide each layer is opened. A layer's target
is clipped to its parent's, so the outer layer has to cover everything the rest
of the chain needs -- and that is not one question but two. A blur or a dilation
grows the image in place, so its output contains its input and the distinction
never shows. A matrix filter *moves* the image, and the inner layer draws the
shape where it was written: the composite is what moves it. A target sized for
where the filter put things crops the content before the filter ever runs. So
the region is followed through the chain as a region, taking the union at each
step, rather than padded out by a margin.

The region is followed in device pixels rather than in the caller's
coordinates. Every filter's reach is a length in device pixels, and mapping one
back through the transform to have it mapped forward again is not the identity
under a scale -- the bounds are floored and ceiled at the end, and a reach
divided and remultiplied is the same length only if nothing rounds.

**A mip chain is stated when a texture is created, not made for every one.**
It costs a third again in memory -- each level is a quarter of the one above,
and a quarter summed forever is a third -- and a pass of downsampling on every
upload. Most textures here are drawn at or above their own size, where no level
below the first is ever read, so making one unconditionally would be paying that
on every image for the benefit of the few that minify. A caller who will minify
says so, and the level count follows from the size: the longer axis halved until
it reaches one texel, with the shorter axis holding at one while the longer keeps
going.

The levels are filled by the backend when the texture is written, and a caller
cannot hand them in. That is not an omission. A chain built by any rule other
than the backend's own would sample differently on the two backends, and the
whole test model here is that they do not -- so the one thing a caller could
supply is the one thing they must not.

The two backends spell the same chain differently, and only one of them has a
choice. GLES has `glGenerateMipmap`, which is the driver's own downsample, and
asking for anything else would be inventing a disagreement. Vulkan has no such
call, so the chain is a blit per level: level two is the average of level one
rather than a quarter-scale filter over level zero, which is what a chain means
and what differs once an image has any detail near its own resolution. The
barriers there are the one place in this backend that transitions a subresource
rather than an image, and have to be: each blit reads the level above while
writing the one below, so the same image is a transfer source and a transfer
destination at once. Everywhere else a transition covers every level, because a
chain left half in the old layout is a validation error waiting for the first
minified draw.

Three separate things have to be right before a single level below the first is
ever read, and each of them fails silently on its own. The image needs the
levels. The sampler needs a maximum level of detail -- it defaults to zero,
which clamps every read back to the largest level. And the view needs to span
the levels, since a view over one of them is one a sampler cannot minify
through. All three failures look identical from inside the renderer: the chain
is there, the sampler is willing, and every read still lands on the image
itself. There is a mutation test for each.

Which level to read is the fragment's own arithmetic, from the screen-space
derivative of the coordinate in texels. The derivative is taken from the
coordinate *before* tiling: a repeat wraps with `fract`, whose derivative at the
seam is the width of the whole image, and a level chosen from that is the
smallest in the chain -- a blurred line down every seam. The branch that reaches
the derivative is on a value from the uniform buffer, so the control flow is
uniform across the draw and the translator's own analysis accepts it there.

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

**A scene can name a glyph run, so text is compared across backends.** It could
not before, and the consequence was that the whole coverage path -- an atlas
uploaded as a texture, read through the red channel, tinted by the paint -- along
with everything a run gained recently, had only ever run on whichever backend
came first.

A scene names glyphs by index into a synthetic fixture set for the same reason it
names no texture: it must describe a picture without a device, and a font file is
a device of its own, one whose version decides what the picture is. Four glyphs
is enough for a run to be a run -- solid, half-covered, a ring and a wedge --
which between them cover full coverage, partial coverage, a hole, and an edge
that is neither horizontal nor vertical. What that leaves out is shaping, which
is out of scope here and is the only part of Impeller's text file that is.

The atlas arrives at slot one and the sheet stays at zero, so the number a plate
reads does not depend on what else the plate draws.

**A glyph atlas holds coverage, not color**, in a single-channel format. The
glyph material reads one channel and scales a solid with it, which is what
antialiased text is; an image paint replaces color instead. The two differ in
the material and in where the coordinates come from, and share the binding
machinery underneath. Storing the same byte four times over works — the shader
reads red either way — and costs four times the memory and four times the
bandwidth to sample it.

The transfer paths had four bytes per pixel written in as a literal, which stays
invisible until a format has one. The size a caller must supply, the size read
back, the channel layout a transfer names, and the component *type* it carries
are all properties of the format, and getting the third of those wrong reads
three texels past the end of every row.

The fourth was found the same way and one format later. `R8Unorm` made the
channel layout a property; the type stayed a literal `UNSIGNED_BYTE`, which is
the same mistake surviving in the half of the statement nothing had exercised
yet. A half-float texture was handed bytes, and ten-bit color needs its four
components packed into a single word rather than four separate ones. Reading
back is not always the same answer as writing: the combination an implementation
must accept for a floating-point attachment is four components of `FLOAT`, so
that path asks for what is guaranteed and narrows afterwards, which is exact
because every value being narrowed came out of a half-float attachment.

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

**A rebuilt atlas is packed in a stated order, not in whatever order a hash
map iterated.** Both rebuilds -- the compaction that discards stale glyphs and
the growth that doubles the sheet -- read the glyphs they are keeping out of a
`HashMap` and reinserted them in iteration order. Rust seeds that per process,
so the same text through the same atlas packed differently on every run.

The layout differing would not matter on its own, since a glyph's coordinates
travel with it. What mattered is that shelf packing is order-sensitive: a shelf
is as tall as the tallest glyph on it, so a short glyph landing first opens a
shelf a tall one cannot use. Measured over five runs of one fixed sequence, the
atlas came out holding thirty-six glyphs three times and thirty-five twice --
whether a glyph fit at all was decided by the hasher, and the comment saying a
repack "cannot fail" rested on packing a subset in an order nothing guaranteed.

Rebuilds now sort tallest first, with width and then the key breaking ties so
that two glyphs of a size land in the same order every run. That is both the
determinism and the standard shelf heuristic, which is why it costs nothing.

What can be tested from inside a single run is narrower than the fault, and the
test says so. Cross-run determinism is not observable where the hasher is seeded
once per process, and the glyph loss it caused is probabilistic -- a test for
that would catch the fault sometimes, which is worse than not testing it. So the
assertion is on the deterministic thing underneath: the tallest survivor is
placed first into an empty sheet, so it opens the first shelf. Nothing stronger
holds, because shelves are scanned for the first that fits and a shorter glyph
placed later does land on an earlier one.

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

Surface formats are chosen sRGB where one is offered, with the linear formats
kept as a fallback. Color is linear inside the renderer and the attachment
format applies the transfer function, and the color space asked for is
`SRGB_NONLINEAR` — which is the presentation engine being told the image already
holds encoded values. An sRGB attachment is what encodes them on write.

This was the other way round, on reasoning that inverted itself: that an sRGB
format would apply the transfer to values already carrying it. Linear values do
not carry it, which is what makes them linear. Presented through a linear
format, mid gray reached the display as 128 where 188 is what it should be —
every window a little over a third too dark, and nowhere announcing itself. The
same mistake had been made in the bundled example, found by looking at its
output; this one had a unit test holding it in place.

Scanout had it too, and the same fix applies for the same reason. A format code
describes how bytes sit in memory and says nothing about what they mean, and a
controller scanning out eight-bit color reads them as encoded — so those codes
map to sRGB image views while the code negotiated with the display is unchanged.
Real hardware accepts the exported buffer either way, which is what says the two
are independent. The ten-bit code is left linear: it has no sRGB variant, and
deep color generally carries its transfer function out of band, so assuming one
would be the same mistake pointing the other way. Present mode falls back rather than
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
tile memory rather than writing it out. A multisampled pass must clear. Seeding
the multisample buffer from a target's existing contents has no reverse-resolve
to do it with on one backend and no legal single-to-multisample blit on the
other, so this is a property of the technique rather than of a backend, and
preserving is refused rather than silently discarding what was there.

A caller meets that restriction as the first thing they write rather than as an
edge case, which is worth saying where the technique is described. Antialiasing
is on by default, a fresh canvas has no background, and a stroked line has only
triangles to antialias with, so the shortest program that draws an antialiased
shape is refused. The refusal names both remedies -- a clear color, or a single
sample -- rather than naming only the copy it could not perform, and
`Canvas::clear` and `Paint::with_anti_alias` each document the other as the two
ends of one choice. The asymmetry that makes this confusing is real and
deliberate: a rectangle, rounded rectangle, oval or circle antialiases inside
its own fragment shader and is never multisampled, so the same program built
from those draws works, and only the tessellated shapes are refused.

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
push-constant equivalent, so per-draw material data is written into one buffer
per submission and bound a range at a time. MSAA uses multisampled renderbuffers with a blit resolve, and
`GL_EXT_multisampled_render_to_texture` on tilers where present.

GLES 2.0 is permanently out of scope; the feature gap is too large.

## Runtime effects

**A runtime effect is not a shader compiled at run time.** This document and
the parity table both said it was, and both were wrong. Flutter compiles these
ahead of time with `impellerc` and ships the result in the asset bundle: the
payload is one already-compiled blob per backend — `sksl`, `metal`, `opengles`,
`opengles3`, `vulkan` — carried in a flatbuffer as bytes, with the uniform
names and descriptor layouts alongside. What happens at run time is that the
engine builds a *pipeline* from a module it did not know about when it was
built.

That is a much smaller problem than runtime translation, and it is the one this
renderer will solve. A caller hands over the payloads their own build produced;
nothing here compiles anything, and no shader toolchain enters this build or
this binary.

**Why not accept WGSL and translate on load.** It is tempting, because one
source of truth is this project's rule everywhere else and the translator is
already a build dependency. It is declined for the first version because it
would link a shader translator into every application that draws a rectangle,
and because it makes the loading path do work whose failures a caller cannot
see until run time. A caller who wants one source can run the same translation
in their own build, which is what the reference implementation's toolchain does
and what this project's own build does.

**The interface is the paint's own uniform block.** A runtime effect declares
the same std140 block every material uses and reads the floats a caller packed
into it. That is sixty-four floats, which is what a material already costs, and
it buys a version with no new descriptor set, no new binding, and no change to
how a draw's uniforms reach the shader.

**A texture came the same way, and for a reason worth noticing.** The obvious
reading of "an effect needs to sample an image" is that it needs a descriptor
set of its own. It does not, because every draw already binds a texture at the
one binding this renderer's shader declares — a placeholder where the material
samples nothing, since a pipeline must have every binding it declares bound
however unreachable the branch reading it. So a program declaring the same
binding gets whatever the draw named, and the machinery carrying it is the
machinery that was already there.

**Several textures did not need a second set either, which was the surprise.**
That was written here as the thing a second descriptor set would be for, and it
is not: a layout may declare bindings a shader never mentions, so widening the
one shared layout to four images serves the solid pipeline unchanged and gives a
caller's program the rest. What a second set would have bought is an unbounded
count, and the ceiling is what buys a single layout instead — raising it costs an
image binding on every draw, removing it costs a layout per program.

The numbering is the part both backends have to agree on. Binding zero is the
first texture, one is the sampler they all share, and two upward are the rest,
so a program written for one backend is written for both: on Vulkan those are
descriptor bindings, and on GLES they are the names naga gives its combined
samplers, `_group_0_binding_N_fs`, which the translator derives from the
texture's binding and which is therefore contractual.

What changed shape is the descriptor sets themselves. There was one per supplied
texture; there is now one per distinct *tuple* of slots the batch asks for, since
a set holds every image a draw reads at once. An ordinary draw contributes a
one-element tuple, so a batch of image draws allocates exactly what it did.

GLES needed one thing Vulkan did not: a sampler uniform defaults to texture unit
zero, so a program declaring two would read one texture twice — a picture that
looks like a binding that never happened. Each sampler is pointed at its unit
once at link time, since that value is program state rather than something a draw
sets.

**A program is a pipeline, not a material kind.** Every material today shares
one fragment shader and picks its behavior by branching on a kind. A runtime
effect replaces that shader, so it is a property of the pipeline instead: the
pipeline key names the program, the context holds the registered ones, and a
draw carries which it uses. This is the first thing in this renderer to make
the pipeline cache hold more than one program, which is also why it is the
change that has to be got right rather than the shader that runs.

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

Built on [drm-rs](https://github.com/Smithay/drm-rs), which is Smithay's own
Rust binding to the kernel interface and not a port of anything. This section
described it as the Rust port of drm-cxx, which was wrong twice over: drm-rs
has no such lineage, and no Rust port of drm-cxx exists. The correction matters
more than a misattributed name, because a boundary drawn against a library that
does not exist can assign it work nobody does — which is what had happened to
one row below.

**The dependency direction is one-way and this crate reimplements no KMS
logic.**

| Concern | Owner |
|---|---|
| Device open, master acquisition | drm-rs |
| Seat and session handoff | the application or its session manager; not drm-rs |
| Connector, CRTC, and plane discovery | drm-rs |
| Mode selection policy | the application; drm-rs reports the modes |
| Atomic commit construction, page-flip events | drm-rs |
| Hotplug detection | nobody here: it needs udev, drm-rs does not provide it, and this crate does not do it |
| dma-buf to framebuffer import | drm-rs |
| HDR metadata, VRR, plane rotation properties | drm-rs, through generic property access rather than a typed surface |
| Buffer allocation, image import and export | impeller-present-drm |
| Frame pacing against flip completion, fence plumbing | impeller-present-drm |

The rows this crate does not own are not aspirations: `drm::control::Device`,
`ClientCapability::Atomic` and `UniversalPlanes`, connector state, `DrmFourcc`
and `DrmModifier`, and `receive_events` with `PAGE_FLIP_EVENT` are what it
calls today, and the vkms lane exercises them without a display.

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

naga output is snapshotted in the repository, so a naga upgrade that changes
codegen is a reviewed event rather than a silent behavior change. The snapshot
is a test rather than a CI-only diff, which is what makes it run everywhere the
suite does.
The GLSL is stored whole because it is text somebody can read a diff of; the
SPIR-V is stored as a word count and a hash, which notices a change and is
honest about not being reviewable.

That check is for the case nothing else here covers. Every other comparison in
this workspace holds the pixels against another implementation of the same
translator, so codegen that changed in the same way on both targets passes all
of them.

**Specialization is not implemented.** The intent is one set of declarations in
the WGSL source mapping to spec constants on Vulkan and bounded build-time macro
permutations on GLES; there are no spec constants anywhere yet, and the fragment
stage dispatches on a material kind at runtime instead — a chain of comparisons
every fragment walks, which is the cost specialization would remove.

## Renderer internals

- **Geometry** (`impeller-geometry`): lyon for general fills and strokes;
  convexity detection for a fan-fill fast path; Wang's-formula adaptive Bezier
  flattening with transform-aware scale.

  A rounded rectangle asked for as an antialiased solid fill is drawn from a
  distance field rather than triangles: two triangles' worth of geometry
  whatever the radius, against an outline flattened to a tolerance, and an edge
  that antialiases from the distance it already computes rather than from four
  samples. Its geometry is in the shape's own space, with the same
  clip-to-local mapping the gradients carry, because a distance measured in
  clip space would round the corners by different amounts on each axis of a
  target that is not square. Anything else tessellates — a stroke is a
  different shape, a gradient or an image would need its own mapping and this
  one at once, and an aliased fill is asking for the hard edge tessellation
  gives.

  A circle takes the same field, because it is the same shape: a square whose
  corner radius is half its side has no straight edge left, and the expression
  reduces exactly to the distance from the center less the radius. So a circle
  costs two triangles and needed no shader of its own — and comes out nearer a
  real circle than the tessellated one does, which is a polygon approximation
  with its coverage quantized to however many samples the pass has.

  It also means such a shape does not multisample the pass. Antialiasing used
  to be a property of the frame, since a request from any shape turned it on
  for all of them; a shape that computes its own coverage now leaves the pass
  at one sample, which is four times less fill and bandwidth for an edge it
  was already going to get right.

  Only where the blend composites, though. The shape is drawn on a quad larger
  than itself and emits a fragment everywhere on it, including where coverage is
  nothing — so a mode that discards the destination for a transparent source
  erases what is behind it in the gap between the two, and in a rounded corner
  that gap is most of the corner. Tessellating covers only the shape and has no
  such gap. The condition falls out of the blend factors rather than being a
  list: the destination's factor has to be `One` or `OneMinusSrcAlpha`, since
  the source term vanishes with its alpha whatever weight it carries. The same
  condition is what makes a partly covered edge correct rather than merely
  harmless, because weighting a premultiplied source by coverage then
  compositing gives what multisampling would have produced.

  A stroke of any of them takes it too. A field already says how far every
  fragment is from the edge, so an outline is the band where that is small: one
  more subtraction and no vertices, against a tessellated outline which is a
  second shape built offset inward and outward with its corners resolved. The
  joins and caps a stroke style carries are ignored rather than refused, since
  these are closed curves with no corners to join and no ends to cap, which is
  what lets a stroked one take this path at all.

  A band's coverage is the difference of two edges', so it inherits both their
  errors — measured, on one pair of devices with the same three shapes: a
  single edge differs by three, a band around it by seven, and a band with
  gentle curvature by one. That is what the distance-field budget is set from,
  and it governs only whether two devices agree. A shape that is the wrong
  shape is wrong on both of them equally, and is caught instead by holding it
  against the tessellated route and against the exact area it approximates.

  A plain rectangle is the same field with no corner to round, and takes it for
  the second reason rather than the first: four vertices is four vertices
  either way, but the pass no longer multisamples for it. That is the shape a
  frame is mostly made of, so it is where the saving is largest — and it is
  invisible on an axis-aligned rectangle at integer bounds, which has no edge
  to antialias, and worth the most on a rotated or fractionally placed one,
  which is what needed four samples before.

  Where the time actually goes was measured rather than assumed, and it is not
  where the vertex counts suggest. At 1920×1080 with a hundred and sixty
  rounded rectangles, on a desktop discrete part: the distance field at one
  sample takes 0.26 ms, the same shapes tessellated at four samples take
  0.64 ms, and tessellated at one sample they take 0.20 ms.

  So the whole of the gain is in not multisampling. At equal sample count the
  field is about thirty percent *slower* than the triangles it replaces — a
  desktop part chews through those vertices nearly for free, and evaluating a
  distance and its derivative is fragment work the triangles do not do. Seven
  times fewer vertices is a real reduction and is not what pays here; being
  able to leave the pass at one sample is.

  That balance is hardware-dependent in the direction this project cares about.
  On a tiler, multisampling resolves in tile memory and costs far less than it
  does here, while vertex and binning work costs more — so the margin should be
  expected to narrow and could invert. Nothing on this machine says which, and
  it should be measured on a board before the field is assumed to be the faster
  path everywhere.

  `cargo xtask bench` is that measurement, and it is repeatable now rather than
  a number somebody once took. On the machine this was written on it reproduces
  the shape of the figures above: at equal sample count the field is
  twenty-six to fifty percent slower than the triangles depending on the
  device, against the thirty percent recorded here, and it still beats the
  multisampled alternative on all three. The absolute times differ, as they
  should — that is a different part.

  One thing the figures above do not say, and the harness had to be changed to
  report: **the two paths do not cost the same number of draws.** Every
  tessellated shape here carries one solid material, so a batch merges all
  hundred and sixty into a single draw, while an analytic shape carries its own
  geometry inside its material and can merge with nothing. So this is one draw
  of many triangles against a hundred and sixty draws of two, and the tessellated
  side is flattered by a merge that real content — where the shapes differ in
  color — would not get. The comparison is still the right one to make, because
  it is what the renderer actually does with each; it is not a comparison of
  shading cost, and reading it as one would be reading it wrong.

  An ellipse takes one too, by a different route. There is no closed form for
  the distance to an ellipse — but coverage never needed a distance. What it
  needs is how far away the curve is in pixels, and that is the implicit
  function divided by how fast it changes across a pixel: zero on the curve,
  and correct to first order either side of it, which is the only place
  coverage is between nothing and all of it. No iteration and no intermediate
  distance.

  Forming the distance first and then normalising it does work, and was tried:
  it approximates twice, and two devices need not make the same error at each
  step. That version diverged across devices by six units where this one
  diverges by two, on top of being longer and costing an extra square root.

  A tessellated rounded rectangle does at least reach the fan fill, since its
  flattened outline is convex, so it skips the sweep and the stencil.

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
- **Entity layer** (`impeller-entity`): **coverage only, and nothing routes
  through it.** The design is that an entity carries transform, blend, clip
  depth, contents, and geometry, with a `Contents` implementation per material
  and coverage computation for culling. The canvas still records into a batch
  directly, so the layer sits beside the pipeline rather than in it, and the
  reason for that has not changed: building the rest before there is a caller
  would fix its shape around a guess.

  Coverage is built because it is the part that is not a guess. The canvas
  computes it today in four places by hand — bounds grown by a stroke, a
  layer's outward reach, what a mask blur covers, and the device bounds a save
  layer is sized to — and each is the same composition written again. It is
  stated once here, in order: geometry bounds, carried through the transform,
  grown by how far the contents reach, then narrowed by the clip.

  Both ends of that order are load-bearing and both are tested by making the
  other order fail. A filter's reach is a distance on the target, so growing
  before the transform would send the reach through it and a shape drawn at ten
  times the scale would blur ten times as far. And the clip narrows last
  because it applies to the finished picture: clipping before growing lets a
  blur back out past the clip, and the same coverage sizes an offscreen target,
  which would then be too large by the reach on every side.

  Coverage is three cases rather than a rectangle. Nothing drawn has no bounds
  and must not read as a zero-area rectangle at the origin, which is a real
  place that a collapsed transform produces and that a caller must not cull.
  And `drawPaint` covers whatever the clip admits, which stays unbounded until
  a clip resolves it — a very large rectangle would be a number somebody chose,
  and wrong at some scale.
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
  an error on Vulkan and a wrong picture on GLES.

  A blurred layer is multi-pass and separable: the contents into a target, then
  one pass per axis, then the same composite an unblurred layer uses. Separable
  because a two-dimensional Gaussian is the product of two one-dimensional ones,
  so two passes give the same picture as a square of taps — at a radius of
  sixteen, thirty-three taps against a thousand and eighty-nine. Keeping the
  blur passes free of alpha and blend means neither has to know about
  compositing.

  A shader loop is bounded, so past a sigma of about eleven the taps no longer
  cover three deviations. They spread rather than truncate: the same count,
  further apart, still spanning the curve, with the sampler's bilinear filter
  averaging what falls between them. That trades quality for coverage, which is
  the right way round — a slightly under-sampled wide blur looks like a wide
  blur, while a truncated one stops softening however large sigma grows and
  creeps toward a box, which is the wrong answer at exactly the sizes a frosted
  panel or a large shadow asks for.

  A blur reaches past what it is given, so a layer that is both bounded and
  blurred outsets its target by three deviations, matching where the shader
  stops taking taps. The caller states where the content is, which is the
  question they can answer; how far the blur carries it is arithmetic that
  belongs here. Sized to the content alone, the halo is cut off square at the
  bound — which looks like a shadow drawn with a straight edge rather than like
  anything to do with bounds.
- **Text** (`impeller-text`): shelf packing of caller-supplied coverage into
  one texture, with compaction and then doubling when it fills. Rasterization
  is out of scope and lives with the caller's font parser, which is what lets
  the atlas be tested against bitmaps whose contents are known exactly.
  **Not implemented**, and named because this list claimed them: subpixel
  quantization, SDF above a threshold size, COLR/CPAL color glyphs, and
  paging — a page boundary would split a glyph run into more than one draw,
  which is why growth was chosen over pages.
- **Color**: linear f32 internally, in sRGB primaries with no range limit, with
  sRGB conversion at the API boundary and at target write. Linear internally
  because blending and interpolation are operations on light: averaging two
  encoded bytes is not averaging the two colors, and a gradient built that way
  is visibly wrong in its middle. The conversion on the way out is the target
  format's, not a shader's — an sRGB format encodes on write, so nothing in the
  pipeline knows the difference and a caller picks it when creating the surface.

  *No range limit* is the second half of that policy and is stated separately
  because it is a different claim. A color states which primaries it is against,
  and one stated against Display P3's is carried in sRGB's — which puts
  components outside zero to one, since the sRGB primaries describe a smaller
  triangle and saying so takes a number outside it. That is extended sRGB, and
  it is what upstream uses as its intermediate space too. Nothing between the
  paint and the target clamps: a floating-point target holds what arrives, and
  an eight-bit one clamps at the write, which is the target format's business
  exactly as the transfer function is.

  Two places still hold a color inside the triangle, and both are stated rather
  than left to be found. The advanced blend modes are defined by the compositing
  specification on components between zero and one and are not defined outside
  it — that is the specification's domain rather than this renderer's, the
  fixed-function unit a paint's blend mode reaches has the same one and cannot
  be extended, so operands are brought to the edge before a mode is evaluated
  rather than fed to a formula with no answer. And a bicubic image read clamps,
  because a kernel with negative lobes invents values no texel it read contains:
  at the point it would have to be decided, a ringing overshoot and a color
  outside the triangle are the same number, so suppressing the first suppresses
  the second with it.

  **Presenting a wide gamut is not built.** The swapchain negotiates
  `SRGB_NONLINEAR` and the scanout path is untouched. The pipeline is verified
  to an offscreen floating-point readback and no further, because the devices
  available are a software rasterizer and a virtual display controller — a
  Display P3 surface cannot be exercised here, and code whose correctness rests
  on having read a specification is not what this project ships.

  A caller picks the format when creating the surface, and picking the wrong
  one is not an error — it is an image that is arithmetically correct and looks
  wrong. The bundled example rendered into a linear surface and wrote the bytes
  to a file for a long time, which every viewer then read as sRGB: the result
  was uniformly far too dark, and nothing about it said so.

  The property that ties the two together is a round trip: a color authored
  through `Color::srgb` and drawn into an sRGB surface comes back as the byte
  that was authored. That holds exactly on both backends, and a test also
  requires the same drawing into a linear and an sRGB surface to differ by
  exactly the transfer — which is what says the pipeline carried light the
  whole way and only the final write encoded, rather than converting early or
  twice.
- **Materials**: a paint resolved for a backend — color or gradient, already
  in clip space, and the color filter applied to what it produces — packed
  into 256 bytes. It was 128 for a long time, which was exactly the
  push-constant size every device guarantees; the color filter is what grew it,
  and the move to a uniform buffer is what let it. Perspective grew it again,
  by the eight floats between a two-by-two mapping and a three-by-three, and
  that widening cost nothing in practice: a material is padded per draw to the
  device's `minUniformBufferOffsetAlignment`, which is 256 on a great many
  parts, so 224 was already occupying 256. The bound that remains is
  `maxUniformBufferRange`, whose guaranteed minimum is 16 KiB. The limit is a
  compile-time assertion rather than a test, and the size stated here is
  checked against it.

**A degenerate gradient takes the limit of the real one, not whatever the
arithmetic falls out as.** A radial gradient's radius is folded into the matrix
that maps a clip position into the gradient's own space, so the shader measures
against unit distance and never sees a radius. A radius of nothing makes that
matrix singular, and the inversion answers a singular matrix with the identity
-- which is the right answer for an inversion and the wrong one here, because
the identity is *a* radius: one clip unit. A gradient asked to have no extent
came out spanning half the target, and would have spanned a different distance
on a target of another size. Nothing failed; it drew a plausible picture nobody
asked for, which is the failure this document keeps warning about.

The answer is the limit of the shrinking gradient. Every point but the center
runs off the end of the ramp, so clamp settles on the last stop and decal draws
nothing, because past the end is where decal draws nothing. Repeat and mirror
have no limit -- the parameter oscillates faster and faster -- and take clamp's
answer, since a stable color is worth more than an arbitrary one that shimmers.

The limit rather than a refusal, on the same reasoning that makes a mask blur of
zero the sharp shape and a morphology of zero the unfiltered one: a caller
animating a value down to nothing should arrive somewhere, not have the thing
disappear on the last frame.

**A gradient locates itself from an interpolated clip position, not from the
fragment coordinate builtin.** That builtin's origin differs between the two
graphics APIs, so using it would run gradients in opposite directions on each
backend. Clip space is normalized by shader translation and agrees everywhere,
and passing it as a varying also avoids a second vertex attribute that every
solid draw would otherwise pay for. Gradient endpoints travel through the same
transform the geometry does, so a gradient rotates and scales with its shape
rather than staying pinned to the screen.

## Coverage against `dart:ui`

What this renderer owes an answer to is the `Canvas` and `Paint` surface Flutter
draws through, not any particular implementation of it.
[`parity.md`](parity.md) is that comparison, row by row, with each claim of
support naming the corpus scene or test that renders it. It is deliberately not
a comparison against the C++ Impeller's internals: matching another
implementation's structure is how a reimplementation acquires its design without
its reasons.

## Crate layout

| Crate | Owns |
|---|---|
| `impeller` | Public facade; carries the feature flags |
| `impeller-core` | Public API: Canvas, Paint, Path, Recording, glyph runs |
| `impeller-entity` | Entity and Contents layer — **coverage only**; nothing routes through it |
| `impeller-geometry` | Path types, flattening, tessellation, stroking, dashing |
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
| `xtask` | Capability and scanout reporting, the skip census, the contact sheet; device runs against real boards and golden management planned |

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
to aarch64 and riscv64 boards and containerized CI builds trivial. CI checks
both of those targets on every push, with `check` rather than `build`: linking
would want a cross linker and a target C runtime, and needing neither is the
claim being made. `deny.toml`
describes this as a ban on `pkg-config`, `cmake` and `cc`, and describes it as
automated enforcement, which it was not: nothing invoked cargo-deny. It is now
enforced by a test that reads the workspace's dependency graph and fails on any
crate whose job is to compile, locate, or generate bindings to something that is
not Rust. That needs nothing installed, which matters for a rule whose whole
value is that it holds on a machine nobody has configured.

**Which Rust toolchain is pinned, and to what.** `rust-toolchain.toml` names
1.94.1, and the number is not a preference: it is what Yocto's wrynose release
ships, from `rust_1.94.1.bb` in openembedded-core. Building here with the
compiler the embedded target will use is what turns a version incompatibility
into a failure on a developer's machine rather than one found in a bitbake
build. The pin has no effect on a bitbake build itself, which uses the
toolchain its own recipe provides and never reads this file.

What moves the number is the deployment target moving, not a new release
appearing upstream: it tracks whichever Yocto release this is meant to run on,
so it follows that recipe rather than the calendar. Moving it means running the
gate under the new version, and the shader snapshots are the part worth looking
at rather than trusting -- not because the compiler translates anything, but
because a Yocto release that moves Rust tends to move `naga` too, and that does
change the emitted SPIR-V.

What decides whether a Yocto release can build this at all is `rust-version`
in the workspace manifest, which is separate and deliberately lower. At 1.85 it
also admits whinlatter's 1.90, checked. Walnascar's 1.84.1 misses by a single
release, and not on anything written here: a transitive `getrandom` needs
edition 2024, which stabilized in 1.85.

Scarthgap's 1.75 is the LTS and so worth knowing the price of rather than
guessing at, and the price is not what the ten-release gap suggests. Measured
rather than assumed: every crate in the workspace compiles on 1.75, and what
stands between is four things, three of them mechanical. The lockfile has to be
version three, which the newer resolver will produce when asked for a lower
minimum. Two `c"..."` literals in the Vulkan backend want 1.77, and one
`Option::is_none_or` in the GLES one wants 1.82; both have plain equivalents.

The fourth is not mechanical. `naga` 23 floors at 1.76 across the whole line,
so the shader pipeline would have to move back to 22 -- and the snapshots say
that is not a free substitution. The same shader translates to the same number
of SPIR-V words and a different hash, so the emitted code differs and would
need validating on hardware rather than assumed equivalent. That, and not the
language version, is what supporting the LTS actually costs. The test targets
are a separate matter again, where an older `googletest`'s `Result` alias takes
one parameter where the current one takes two.

A pinned toolchain also makes the gate mean the same thing everywhere, which
matters here more than it would elsewhere: the lint step denies warnings, and a
floating compiler turns a new lint into a red build on an unrelated change.

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

**A feature a scene asks for has to change the picture.** Comparing two
backends says they agree, and two backends agree perfectly about a feature both
of them ignore — so the comparison is silent about exactly the failure it looks
like it would catch. Every scene carrying an image filter, a mask blur, a
layer's blur or a mode combining a carried color renders twice: once as
written, once with those taken away, and the two must differ. It found one
scene on its first run. `blur/can-render-backdrop-blur` put its layer over a
flat background with nothing behind it, and a backdrop blur of one color is
that color, so a plate named for the filter had never been able to show it.

**The suite is near a GPU memory ceiling.** `cargo test --workspace` runs test
binaries concurrently and each holds its own context, so peak allocation is the
sum across them rather than the largest. A run has been seen to fail with
`out of memory allocating texture memory` in one binary while another was
rendering, and to pass on the next attempt with nothing changed. It is memory
pressure rather than a leak — every path frees what it takes, and a binary run
on its own has room to spare. Worth knowing before reading such a failure as a
logic error, and worth watching: the headroom shrinks as the corpus grows.

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
fragment.** A solid fill copies a color through the pipeline, and any
difference there is a defect. A gradient evaluates one, a blend converts an
intermediate to fixed point, and a multisample resolve averages — none of which
the specification requires to be bit-identical, since shader arithmetic is
permitted some error and compilers may fuse operations differently. Deriving it
means a new scene inherits the right rule instead of acquiring a hand-set
number, and a genuine divergence cannot be waved through by loosening one entry.

Coverage computed from a distance field gets a third, bounded the opposite way:
a few units everywhere and no outliers. The width of such an edge comes from a
screen-space derivative, which both specifications leave to the implementation —
it may be evaluated once per two-by-two quad or by differencing neighbours —
so two devices differ by a unit or two along the whole edge rather than by a
sample's worth at a few pixels. Bounding it by magnitude keeps it able to tell
that from a defect, since a shape in the wrong place moves edge pixels by far
more. The two backends on one device agree exactly, which is what says this is
device arithmetic rather than logic.

Multisampling gets a budget of a different shape, still derived. Neither
specification says which samples an edge covers when it passes near a sample
point, so two rasterizers may include a different one — and the resolve then
differs by a whole sample's share, a quarter of full scale at four samples,
which no per-channel allowance for rounding could admit. It is bounded by *how
many* pixels instead: a thousandth of the image, against the two to four a
curved edge actually produces. Anything systematic moves far more than that, so
the budget still tells a rasterization tie from a divergence — which is checked
directly, rather than assumed, by requiring it to refuse a difference over one
percent of an image.

Where per-driver tables become necessary, tightening one is a normal change and
**loosening one requires a linked driver-bug issue**.

The levels below are the plan. The right-hand column says where each runs.
`ci/smoke.sh` does the whole of L0, L1 and L3 and part of L4, and runs both by
hand and on every push through `.github/workflows/ci.yml`.

That workflow runs on a machine with no GPU, which is worth explaining, since
the obvious reading is that it therefore verifies nothing. Mesa ships
conformant CPU implementations of both APIs targeted here — lavapipe for
Vulkan, llvmpipe for GLES — and the suite runs on them whole, not in a reduced
mode. Pointing the suite at it the first time found two failures, and neither
was the software device's fault: the GLES sample-count mask was synthesized
from `MAX_SAMPLES` rather than queried, and a test required a tiled buffer
layout from a rasterizer that has no tiling.

How much a software device covers depends on which one. A recent lavapipe
offers advanced blending, which the discrete part this was developed against
does not, so it reaches scenes the workstation reports as unavailable; the
older Mesa on the hosted runner does not, and reports those scenes as skipped
in the same census everything else appears in. Neither is a property to assume
from the word "software" — read the run.

What a hosted runner cannot do is KMS, since there is no display controller to
become master of, and it cannot speak to real hardware. Those are the levels
that still need a machine with a GPU and a free connector, and running them
there is still manual.

**A context's own teardown is validation-checked, which took a handle that
outlives it.** Everything else reads the validation log through the context
that owns it, and that arrangement cannot cover the last thing the layer has to
say. A child object outliving its device is reported at `vkDestroyDevice`, and
that call is inside the context's drop -- so those reports were not merely
unchecked, they were unreachable from a test, and the whole class of fault was
structurally invisible. A descriptor set layout leaked on every device for as
long as the material set has existed, with the suite green throughout, and it
was found by running the layer by hand rather than by anything here.

The log is behind an `Arc` already, so the fix is an accessor that hands out a
clone. The messenger is destroyed after the device rather than before, which was
already true and is what makes those reports reach the callback at all.

Where that clone is read decides how much it covers. A dedicated test that holds
one across a drop covers one context. Putting it in the `Validated` wrapper's
own drop covers every context in the suite -- but only if the context is
destroyed *before* the check rather than after, and a field is dropped after its
owner's `Drop::drop` returns. So the wrapper holds the context in a
`ManuallyDrop`, destroys it explicitly, and reads the log afterward. That is the
whole reason for an awkward construction in a type that is otherwise a newtype,
and it is the difference between one test asserting this and every validated
test asserting it: with the leak reinstated, an ordinary drawing test in the
Vulkan backend fails with the layer's own message.

The GLES wrapper is written the same way, and there the deletions it covers are
its own -- program, buffers, placeholder -- which happen while the context is
still current and so can still raise something the callback sees.

The test has to draw before it drops. The objects worth checking are the ones a
context builds lazily -- the leaking layout is created on the first draw that
needs a material -- so a context that never drew would pass while the fault was
live.

**The workstation cannot run everything the suite can test, and the command
that closes the gap says which device it chose.** Neither device here offers
advanced blending -- not the discrete part through Vulkan, not the same part
through GLES -- so every scene needing it is skipped locally and runs only on
the hosted runner, which has no GPU and uses Mesa's CPU drivers. That is not a
reduced mode: lavapipe and llvmpipe are conformant, and the runner covers
strictly more than this machine does.

The consequence is that a failure only the runner can see is one this machine
cannot reproduce, and one of those has already happened. A check on whether a
catalog scene drew anything compared every pixel against the first, which reads
a correctly-uniform picture as blank -- and the plate that produces one needs
advanced blending, so it was skipped here and stayed wrong for twenty-seven
commits while every local run went green.

`cargo xtask gate --software` and `cargo xtask verify --software` select the CPU
drivers, and they set *both* variables or neither. Setting only the Vulkan one
is the obvious half and gives a differently wrong answer rather than a partial
one: the cross-backend comparison then holds a software rasterizer against a
hardware one and reports every antialiased edge as a disagreement. The ICD
manifest is found rather than named, because its architecture suffix differs
between distributions and CI already got that wrong once by hardcoding Fedora's,
and the match is on the driver's short name with a test that none of the dozen
other manifests in that directory answers to -- selecting radeon's would run
against hardware while announcing that it had not.

**The suite runs with the validation layer installed for every context, not
only the ones that ask for it.** A context that asks installs a debug messenger
and routes what the layer says into a log its own tests assert on. A context
that does not ask -- which includes every one the public API creates, the path a
caller actually takes -- had no validation at all, and that is where a leaked
descriptor set layout hid on every device for as long as the material set has
existed.

So the census sets the loader variable that installs the layer process-wide.
Without a messenger the layer writes to stderr, the census already captures
both streams, and a scan for its objections turns them into a non-zero exit.
They are counted by their VUID rather than by their text: the layer names the
device and object involved, both of which differ every run, so counting the raw
message would report one fault as one per context. Reinstating that leak makes
the census report a hundred and thirty-nine occurrences of one identifier,
which is one fault and every context in the suite.

An objection is reported as a *break* rather than as a failure, on the same
reasoning as a lost summary line: every test may have agreed about the pixels,
and what the layer saw is about what the process did to the device. A census
that printed "passed" and stopped would be telling the truth and hiding the
important part. If the layer is not installed at all the loader ignores the
variable, which is not a silent pass -- the several tests that need it report a
skip, and naming skips is what this command is for.

The GLES side gets no equivalent, and the reason is worth stating rather than
leaving as an apparent gap. GL has no object tracking: nothing reports that an
object outlived the context that made it, because destroying an EGL context
releases everything it owns and there is no undefined behavior to report. The
same omission existed there -- a placeholder texture created on the first draw
that samples nothing, missing from a teardown written before it existed -- and
was found by looking for it after the Vulkan one, not by any check. So on that
backend "the context releases what it made" is a rule held by reading, and this
document is where it is written down.

**A segmentation fault in the suite was diagnosed to the Vulkan loader, not to
this code.** It appeared twice, in different test binaries, days apart, and
reproduced neither on demand nor under deliberate concurrency on a quiet
machine. The second one left a core, and the core settles it.

Two threads, both inside the loader. One is in `vkEnumeratePhysicalDevices`,
which had reached `loader_unload_scanned_icd` and was calling `dlclose` on
driver libraries. The other is in `vkCreateDevice`, loading device function
pointers through `vkGetDeviceProcAddr`, and it faults in
`loader_get_icd_and_device`. One thread is unloading the drivers another thread
is looking a device up in.

The trigger is this machine having twelve ICD manifests installed. Almost all of
them find no device here, and the loader unloads the ones that do not -- so the
unload path runs on every enumeration, and it is not synchronized against
another thread's device calls. Creating instances and devices concurrently from
several threads is explicitly permitted, and the test harness does it because
each device test takes a fresh context, which is the arrangement in which state
left behind by a previous frame cannot be seen.

Nothing here is doing anything it may not, so nothing here is changed. The
remedy is one variable in the environment of whoever is affected:

```sh
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.x86_64.json:/usr/share/vulkan/icd.d/lvp_icd.x86_64.json
```

Naming the manifests directly is what keeps the loader from opening the others,
and opening them is what leads to unloading them. The paths differ by
distribution; `ls /usr/share/vulkan/icd.d/` is where they are.

Two of them, not one, and that is the part worth stating. The conformance suite
renders the same scene on the hardware Vulkan driver and on the software
reference and compares them, so naming only the first trades a crash for a test
that silently stops comparing anything -- the quieter of the two failures and
therefore the worse.

The name-based selectors look like the tidier answer and are not the answer at
all, which is worth recording because the next person will reach for them.
`VK_LOADER_DRIVERS_SELECT` and `VK_LOADER_DRIVERS_DISABLE` choose which drivers
are *used*, after the loader has opened every manifest it found. Measured on
this machine by counting which driver libraries actually get opened: with
neither set, twelve; with `SELECT` naming two, still twelve; with `DISABLE`
naming ten, still twelve; with `VK_DRIVER_FILES` naming two, two. Only the last
prevents the open, and only preventing the open prevents the unload.

This is not done for you. Naming the files means holding a list of GPU vendors
somewhere, and a renderer's build tooling is the wrong place for one -- it would
go stale on the first machine nobody tested. `cargo xtask gate` names nothing
and inherits whatever the environment says, reporting it when it says
something, so a run records which drivers it was against without deciding them.
The software lane is the exception and is not a vendor guess: `--software` names
lavapipe because lavapipe *is* what that lane means, and it is why that lane has
never crashed.

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

Color is premultiplied everywhere — a render target holds it that way, an
uploaded image is required to, and the blend equations assume it — so no channel
can exceed the alpha it was multiplied by. That is asserted over the corpus, and
it needs a scene that clears to transparent in order to mean anything: at full
alpha the two conventions agree exactly, so over an opaque corpus the check is
arithmetic that cannot fail. It carries a count of partly transparent texels for
that reason, which fails if the corpus ever becomes opaque again.

Some of what a comparison cannot see is still arithmetic, and arithmetic can be
asserted. A scene that clears to an opaque background has no way to become
transparent unless a draw took the alpha away, and none of them mean to — so
every such scene is required to render fully opaque. A hole is invisible to
every comparison here, because it is the same hole on both sides of each of
them, and invisible to a person too while the background is black. It is not
invisible to a count of pixels below opaque.

**No comparison here can see a scene both implementations get wrong the same
way.** Backend against backend, device against device, a scene against a
mutation of itself — all of it is relative, and a gradient banded identically
everywhere, a shape consistently in the wrong place, or a color that is
arithmetically correct and far too dark passes every one. The bundled example
wrote linear bytes into a file every viewer reads as sRGB for a long time, and
nothing in the suite could have said so.

Both the example and the gallery are run by `ci/smoke.sh`, which is otherwise
the suite plus the feature matrix. Neither is covered by a test: they are code
that runs, so a change breaking one compiles, passes everything, and is found
by whoever next runs it. For the gallery that would be worse than an
inconvenience, since it is the only check here that can see a scene both
backends get wrong the same way — a broken one is a check quietly lost.

`cargo xtask gallery` renders every corpus scene onto one sheet, with the grid
printed alongside so a tile can be found by counting. It answers a different
question from everything else here, and only a person can read the answer. It
suits gross wrongness rather than fine judgement: the first two things that
looked wrong on it were not, and measurement said so both times.

A scene the preferred device cannot render is drawn by whichever device can,
and reported as such. Otherwise the holes fall in exactly the places least
looked at — the advanced blend modes need an extension this hardware lacks, so
the six scenes nobody could see would be the six that most reward seeing. A
tile drawn by a fallback is not evidence about the preferred device, which is
why the command says which ones were.

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

**A sweep reports every failure in it, not the first.** Several tests here ask
the same question of a set — every blend mode against its equation, every
drawing call given a coordinate that is not a number — and for those the useful
output is which members fail, since a wrong table usually breaks several and the
pattern is what identifies the mistake. Rust's built-in assertions are all
fatal, so this was hand-rolled four times as a vector of strings and an assert
at the end. It is now the `googletest` crate's `expect_that!` and `expect_true!`,
which record a failure and let the test continue. That crate is Google's Rust
counterpart to the C++ framework, and this is the one thing it offers that the
built-in harness does not; the matchers and fixtures it also provides are not
used, since helper functions and `Drop` already cover those.

What it does *not* provide is the counterpart to `GTEST_SKIP`. A Rust test that
discovers at runtime that it cannot run has no way to report itself skipped —
it returns, and passes. That hole is why the census below exists.

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
<!-- Rejected for L2: generating references through a Rust binding to Skia.
     `skia-safe` either downloads prebuilt C++ binaries or builds Skia from
     source with LLVM, Python and Ninja. Either breaks the rule that every
     dependency compiles from pure Rust source, and the prebuilt path cannot
     cross-compile to the boards this project exists for. Should goldens ever
     be wanted, the way to have them without breaking that rule is a tool
     outside this workspace that emits images, committed as data -- and they
     would answer "does this match Skia", which is not the same question as
     "does this match Impeller". -->
| L3 | Conformance: same corpus, cross-backend and cross-presentation diffs | Every merge (software) | runs, cross-backend and cross-device; cross-presentation only for the offscreen target |
| L4 | Presentation: resize storms, flip pacing, fence ordering, hotplug | VKMS and headless WSI in CI | partial — headless WSI runs on both backends, fence ordering is checked under the validation layer, and five tests drive a real display controller through VKMS wherever a card is present. Not in CI, which loads no such module; no writeback, no CRC, no resize storms, no hotplug |
| L5 | Stress and soak: atlas thrash, layer-depth bombs, leak detection | Nightly and weekly, hardware | none |
| L6 | Performance: micro and full-frame benches with regression gating | Nightly, quiet runners | partial — `cargo xtask bench` times the two rounded-rectangle paths against each other on every device present, which is the one measurement this document rests a design on. No full-frame benches and no gating: it prints numbers and passes whatever they say |
| L7 | Fuzz: path data, scene descriptions, dma-buf negotiation | Continuous background | none |

### VKMS would give the DRM path merge-blocking coverage

**Half of it runs, and not where it would block a merge.** Five tests in
`impeller-present-drm` drive a real display controller through VKMS on any
machine with the module loaded: format negotiation, dma-buf import, an atomic
commit, the render-done fence latching before scanout, several frames flipping
in turn, and a framebuffer the output never imported being refused. They skip
where there is no card, which is what CI is -- its runners load no such module,
so nothing below is merge-blocking today. `cargo xtask drm` reports whether a
given machine could host it.

What is described below and does not run is the half that needs more than an
atomic commit: writeback capture, CRTC CRC cross-checks, hotplug injection, and
the failure injection cases. Those are why the lane is worth finishing rather
than why it is worth building.

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
