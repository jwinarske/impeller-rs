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
directory, so what those rows say is said about this renderer and is not an
artifact of measuring too high. `drawParagraph` is the clear case in the other direction — the
framework lays out a paragraph and the engine sees glyph runs, which is why it
is marked out of scope rather than missing. `drawShadow` is plausibly decomposed above
Impeller as well; I have not confirmed that, and have not claimed otherwise in
its row.

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
| `drawImageNine` | yes | `draw_image_nine`: corners kept, edges stretched along one axis, middle along both | `a_nine_patch_stretches_its_middle_and_keeps_its_corners` |
| `drawPaint` | yes | `draw_paint`, which fills the clip rather than the target — not a rectangle a caller can easily write once a transform is in force | `drawing_the_paint_fills_the_clip_rather_than_the_target` |
| `drawColor` | yes | `draw_color`, which blends and obeys the clip where `clear` replaces and ignores it | `drawing_a_colour_blends_where_clearing_replaces` |
| `drawParagraph` | out of scope | shaping and layout are not this project's; `draw_glyphs` takes a positioned run and an atlas | glyph tests |
| `drawVertices` | yes | `draw_vertices`, with positions, texture coordinates and per-vertex colors | `a_mesh_interpolates_the_colours_its_vertices_carry` |
| `drawAtlas`, `drawRawAtlas` | yes | `draw_atlas`, one draw for the whole batch, each sprite with its own transform and color | `an_atlas_tints_each_sprite_on_its_own_in_one_draw` |
| `drawPoints`, `drawRawPoints` | yes | `draw_points`, in all three modes. A point is a segment of no length, so the cap is the whole shape | `a_point_is_drawn_as_the_cap_it_would_have_had` |
| `drawDRRect` | yes | `draw_drrect`: two contours filled even-odd, which is what makes the inner one a hole | `a_double_rounded_rect_is_a_ring_rather_than_two_shapes` |
| `drawShadow` | yes | `draw_shadow`: offset, blur and alpha all from the elevation, under one light | `a_shadow_falls_below_what_casts_it_and_widens_with_elevation` |
| `drawRSuperellipse` | no | | |
| `drawPicture` | no | — a recording here is tessellated, not a command list; see below | |
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
| `getLocalClipBounds`, `getDestinationClipBounds` | yes | `local_clip_bounds` and `destination_clip_bounds`, conservative and accounting for scissor and stencil clips alike | `the_clip_bounds_narrow_with_every_kind_of_clip` |

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
| `shader` | yes | linear, radial, sweep and conical gradients, images, and a caller's own fragment program | `a_caller_can_fill_a_shape_with_their_own_fragment_program` |
| `colorFilter` | yes | `with_color_filter`: a color matrix, the sRGB transfer function in either direction, and any blend against a constant that is affine in what it blends. Not the advanced blend modes, which the paint's own blend mode covers | `the_gamma_filter_follows_the_curve_at_both_ends_of_it` |
| `imageFilter` | partial | `with_image_filter`: a blur and a matrix, applied to what the paint drew rather than to the colour it computed. Not dilate, erode or compose | `a_matrix_image_filter_moves_what_was_drawn` |
| `maskFilter` | yes | `with_mask_blur` and `with_mask_blur_style`: a blur of a shape's coverage in all four styles. Solid colors only, since the identity it rests on holds for nothing else | `each_mask_blur_style_keeps_the_part_of_the_blur_it_names` |
| `filterQuality` | partial | `with_sampling`: linear and nearest. `medium` and `high` are mipmapped and bicubic, and neither exists here to select | `nearest_sampling_reads_one_texel_where_linear_blends_two` |
| `invertColors` | via | a color filter whose matrix negates each channel and adds one | |

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

Of forty-seven rows across `Canvas` and `Paint`: thirty-five exist, three are
partial, five are expressible by a caller who assembles them, three are absent,
and one is out of scope. Counting them is the least interesting thing
about the table -- the absences are not equal, and a reader deciding whether
this renderer is usable should look at which ones rather than how many.

The two that would matter most to a real application, in the order I would
build them:

1. **`drawPicture`**, which is a smaller feature here than it is in Skia and
   worth understanding before anyone plans it. An `SkPicture` is a command
   list, so replaying one under a new transform re-runs the commands and
   re-tessellates. A `Recording` here is already tessellated: paths were
   flattened at a tolerance chosen from the transform in force when they were
   recorded, and both the vertices and the materials are in clip space.

   Replaying one under another transform is mechanically possible — the
   composite is affine, so clip-space positions and each material's own
   geometry can be carried through it — but the flattening cannot be undone.
   A recording magnified shows the polygon it was flattened to. So this would
   be a convenience for composing scenes at the scale they were recorded at,
   not the reuse optimization the same call is elsewhere, and a caller who
   wants that should re-record. The pass model has the other half of the
   question: a nested recording arrives with its own passes and its own
   texture table, and merging those is about numbering slots.
2. **The rest of runtime effects.** A caller's fragment program draws on both
   backends, through the paint, with the material's own uniform block — fifty-
   six floats — and one texture, which it gets without a descriptor set of its
   own because every draw already binds one. What is missing is *several*
   textures, which `dart:ui` allows and which does need a second set. Worth
   building when something asks for it.

   This row was described here for a long time as needing a shader pipeline
   that compiles at run time. That was wrong, and the correction was most of
   the work: Flutter compiles these ahead of time and ships one payload per
   backend, so what was needed was a pipeline cache that can hold more than one
   program, not a compiler. `docs/architecture.md` has the design.

`drawRSuperellipse` and `clipRSuperellipse` were once described here as shapes
with rules attached, cheap once somebody needed them. That was wrong, and the
correction is worth more than the row. Flutter's rounded superellipse is not a
closed form: each corner is built from a superellipse arc joined to a circular
one, and the superellipse's degree comes from an eleven-entry lookup table
interpolated on the ratio of side to radius, extrapolated beyond it. Matching
that shape means transcribing a fitted table from another project, and nothing
in this repository could check the transcription — there is no reference here
to compare against. Drawing *a* rounded superellipse under that name instead
would be the substitution this renderer refuses everywhere else. So it stays
absent, and the reason is a decision rather than a gap in the work.

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
