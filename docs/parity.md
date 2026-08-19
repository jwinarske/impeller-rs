# Parity with what a Flutter-class renderer has to do

## What this measures, and why not the C++ Impeller

The obvious comparison is against the C++ Impeller in the Flutter Engine, and it
is the wrong one. Its `Contents` taxonomy, its Entity layer, its shader compiler
with reflection — those are one implementation's internal shape, and this
project deliberately does not copy them. Matching a competitor's internals is
how a reimplementation ends up with their design and none of their reasons.

What both implementations owe the same answer to is `dart:ui`: the `Canvas` and
`Paint` surface that Flutter draws through. That surface is enumerable, stable,
and not anybody's implementation detail, so it is what this table is built from.
Rows below are every method of `Canvas` and every property of `Paint`, taken
from the published API rather than from memory.

This says nothing about whether the pixels match the C++ implementation's. That
needs golden images from an engine build and is a separate exercise; see the
last section.

### The level this measures at, which is not Impeller's own

`dart:ui` is not Impeller's interface. The stack is `dart:ui` → the tonic
bindings → `lib/ui/painting` → a DisplayList → Impeller, whose actual input
point is a display-list dispatcher. So the left-hand column here sits several
layers above where this renderer sits, and a row marked absent might be absent
from Impeller too, or might be something the layers above decompose before
Impeller ever sees it.

That is a real mismatch and worth knowing when reading the table. It is
deliberate anyway, for one reason: the question this project has to answer is
whether an application's drawing can be served, and the application draws
through `Canvas`. A table built from a dispatcher interface would measure
agreement with one implementation's decomposition of that surface.

Where it could have mattered most, it does not. `drawVertices`, `drawAtlas`,
color filters and image filters all live inside Impeller's own display-list
directory, so what is missing among them is missing here and not an artifact of
measuring too high. `drawParagraph` is the clear case in the other direction — the
framework lays out a paragraph and the engine sees glyph runs, which is why it
is marked out of scope rather than missing. `drawImageNine` and `drawShadow` are
plausibly decomposed above Impeller as well; I have not confirmed either, and
have not claimed otherwise in their rows.

## How to read it

- **yes** — the operation exists as such, and the evidence column names a corpus
  scene or test that renders it. A row claiming *yes* with no evidence is a bug
  in this table.
- **via** — not offered, but expressible with what is here, and the column says
  how. This is deliberately a different status from *yes*: a caller who has to
  assemble it themselves is doing work the renderer did not do, and counting
  those as done is how a parity table starts lying.
- **no** — absent. No workaround claimed.
- **out of scope** — deliberately not this project's job, with the reason.

Corpus scenes are rendered on both backends and compared against each other and
a software reference, so an evidence entry naming one means the operation runs
rather than merely compiles.

The evidence column is checked by a test rather than by hand: every name in it
has to be a corpus scene or a function that exists, and no row may claim *yes*
with the column empty. That check found five citations pointing at nothing --
four naming local variables inside a test about something else, one a name
truncated to an ellipsis -- and three rows resting on prose or on nothing at
all. The counting sentence below is checked against the table too, for the same
reason.

## Canvas

| `dart:ui` | Status | Here | Evidence |
|---|---|---|---|
| `drawRect` | yes | `draw_rect` | `rect-fill`, `transformed` |
| `drawRRect` | yes | `draw_rrect` | `rounded-rect`, `rounded-rect-analytic` |
| `drawCircle` | yes | `draw_circle` | `circle-fill`, `circle-antialiased` |
| `drawOval` | yes | `draw_oval` | `oval` |
| `drawLine` | yes | `draw_line` | `stroke-caps` |
| `drawPath` | yes | `draw_path` | `concave-polygon`, `self-crossing-nonzero` |
| `drawArc` | via | `PathBuilder::arc` then `draw_path` | `arc-ring-and-slice` |
| `drawImage` | yes | `Paint::image` | `an_image_paint_draws_a_texture_through_the_api` |
| `drawImageRect` | yes | `Paint::with_source_pixels` | `a_sprite_can_be_drawn_from_a_sheet_by_naming_its_texels` |
| `drawImageNine` | no | — nine draws with source rects would do it, but that is the caller writing the operation | |
| `drawPaint` | via | a rect covering the target | |
| `drawColor` | via | `clear` for a whole target; otherwise a rect with a blend | `transparent-background` |
| `drawParagraph` | out of scope | shaping and layout are not this project's; `draw_glyphs` takes a positioned run and an atlas | glyph tests |
| `drawVertices` | partial | `draw_vertices`, with positions and texture coordinates. Per-vertex colors are absent, since the vertex format carries none | `a_mesh_draws_the_triangles_it_names_and_nothing_else` |
| `drawAtlas`, `drawRawAtlas` | partial | `draw_atlas`, one draw for the whole batch. Per-sprite colors are absent for the same reason `drawVertices` has no per-vertex ones | `an_atlas_draws_each_sprite_from_the_part_of_the_sheet_it_named` |
| `drawPoints`, `drawRawPoints` | no | | |
| `drawDRRect` | no | | |
| `drawShadow` | no | — the elevation-to-shadow rule, not just a blurred shape | |
| `drawRSuperellipse` | no | | |
| `drawPicture` | no | — no nested recordings | |
| `clipRect` | yes | `clip_rect` | `clipped-circle`, `shape-clip-and-scissor-together` |
| `clipPath` | yes | `clip_path` | `clip-varies-between-draws`, `shape-clipped-fill` |
| `clipRRect` | via | `clip_path` of `Rect::to_rounded_path` | |
| `clipRSuperellipse` | no | | |
| `save`, `restore` | yes | `save`, `restore` | `translucent-stack` |
| `saveLayer` | yes | `save_layer`, `save_layer_bounds` | `layer-group-opacity`, `layer-bounded` |
| `restoreToCount` | via | `save_depth` and a loop | |
| `getSaveCount` | yes | `save_depth` | `saves_nest` |
| `translate`, `scale`, `rotate` | yes | same names | `transformed` |
| `skew` | via | `concat` of the affine | |
| `transform` | partial | `concat` takes a 2D affine; `dart:ui` takes a 4×4 and so admits perspective | |
| `getTransform` | yes | `transform()` | `save_and_restore_return_the_previous_transform` |
| `getLocalClipBounds`, `getDestinationClipBounds` | partial | `clip()` gives the device-space scissor; a path clip's bounds are not tracked | |

## Paint

| `dart:ui` | Status | Here | Evidence |
|---|---|---|---|
| `color` | yes | `Paint::fill` | `rect-fill` |
| `style` | yes | `with_style` | `rect-fill`, `stroke-polygon-and-curve` |
| `strokeWidth` | yes | `Style::Stroke` | `stroke-polygon-and-curve`, `rounded-rect-stroked` |
| `strokeCap` | yes | `StrokeStyle::cap` | `stroke-caps` |
| `strokeJoin` | yes | `StrokeStyle::join` | `stroke-joins` |
| `strokeMiterLimit` | yes | `StrokeStyle::miter_limit` | `stroke-joins` |
| `isAntiAlias` | yes | `with_anti_alias` | `circle-antialiased`, `curve-antialiased` |
| `blendMode` | yes | `with_blend`, all of Porter-Duff and the fifteen advanced modes where the device offers them | `advanced-blend-*` |
| `shader` | partial | linear, radial, sweep and conical gradients, and images. Runtime effects are absent | `gradient-*` |
| `colorFilter` | no | — an image tint exists, which is the narrowest case of one | |
| `imageFilter` | no | — a layer can blur itself or its backdrop, which is not the same as a filter on a paint | `layer-blurred`, `layer-backdrop-blurred` |
| `maskFilter` | partial | `with_mask_blur`, blurring a shape's coverage. Solid colors only, since the identity it rests on holds for nothing else | `mask-blur-shadow` |
| `filterQuality` | no | — one sampler, linear, fixed | |
| `invertColors` | no | | |

## Beyond the surface

Things here that `dart:ui` does not ask of a renderer, because Flutter does them
above it or not at all:

- **Dashing.** A path effect, applied here by cutting the path before stroking.
- **Gradient tile modes**, including mirror, and any number of color stops.
- **Image source rectangles and tinting**, which is `drawImageRect` plus the
  part of `colorFilter` an icon sheet actually needs.
- **DRM/KMS presentation.** Scanning out without a compositor is a presentation
  target here; in Flutter it lives in an embedder, outside Impeller.

## Where that leaves it

Of forty-seven rows across `Canvas` and `Paint`: twenty-three exist, six are
partial, six are expressible by a caller who assembles them, eleven are
absent, and one is out of scope. Counting them is the least interesting thing
about the table -- the absences are not equal, and a reader deciding whether
this renderer is usable should look at which ones rather than how many.

The three that would matter most to a real application, in the order I would
build them:

1. **`colorFilter`** — tinting anything rather than only an image. It is asked
   for constantly and it is blocked rather than unbuilt: the push constants are
   exactly full, and unlike the other rows a color filter has to sit *on top
   of* whatever material is already there, including a four-stop gradient that
   uses every float. So it waits for material data to move to a uniform buffer.

   This row previously claimed that conical gradients waited on the same move.
   They did not: the flag saying a gradient's colors had been baked into a
   texture occupied a whole float, and a material carrying that texture has no
   stop count to report, so the two were folded into one number and the float
   that freed is the one a conical gradient needed. The lesson is narrower than
   "the budget is full" — it was full of one thing that was not paying for
   itself, and that is worth checking before concluding a mechanism has to
   change.
2. **Per-vertex colors**, which is the one thing left in both `drawVertices`
   and `drawAtlas`. A mesh can be drawn and can read a texture at coordinates
   its vertices carry, and a sprite batch is one draw; what neither can do is
   give each vertex, or each sprite, a color of its own. That is not an API gap but a
   vertex-format one -- position and texture coordinate are all a vertex holds,
   and a third attribute is paid for by every solid fill in every frame unless
   it comes with a second pipeline. Which of those two is right is the decision
   this row is waiting on, and it should be made against a measurement rather
   than in the abstract.
3. **Runtime effects** — user fragment shaders. The largest by far: it needs a
   shader pipeline that compiles at runtime rather than at build time, which is
   a different arrangement from the one here.

`drawShadow`, `drawRSuperellipse` and `clipRSuperellipse` are shapes with rules
attached rather than rendering problems, and are cheap once somebody needs them.
`drawPicture` needs nested recordings, which the pass model would have opinions
about.

## What this table does not tell you

It counts operations, not correctness, and not agreement with the C++
implementation. Two renderers can both offer `drawRRect` and disagree about
every antialiased pixel of it. What is checked here is that each *yes* row
renders on both backends and matches a software reference within a stated
tolerance — which is a claim about this renderer's internal consistency, not
about Flutter's output.

Comparing against the C++ Impeller's own goldens would settle that, and needs an
engine build to produce them. It is worth doing and is not this document.

It also says nothing about performance, memory, or how any of this behaves on
the embedded targets the project exists for.
