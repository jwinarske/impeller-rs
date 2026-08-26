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
| `drawRRect` | yes | `draw_rrect` for one circular radius, which keeps the fragment-evaluated route; `draw_rrect_with_radii` for the eight numbers `RRect` carries, which tessellates. Radii that overrun a side are scaled together by `dart:ui`'s rule | `rounded-rect`, `rounded-rect-analytic`, `each_corner_of_a_rounded_rectangle_can_have_its_own_radii` |
| `drawCircle` | yes | `draw_circle` | `circle-fill`, `circle-antialiased` |
| `drawOval` | yes | `draw_oval` | `oval` |
| `drawLine` | yes | `draw_line` | `stroke-caps` |
| `drawPath` | yes | `draw_path` | `concave-polygon`, `self-crossing-nonzero` |
| `drawArc` | via | `PathBuilder::arc` then `draw_path` | `arc-ring-and-slice` |
| `drawImage` | yes | `Paint::image` | `an_image_paint_draws_a_texture_through_the_api` |
| `drawImageRect` | yes | `Paint::with_source_pixels` | `a_sprite_can_be_drawn_from_a_sheet_by_naming_its_texels` |
| `drawImageNine` | yes | `draw_image_nine`: corners kept, edges stretched along one axis, middle along both | `a_nine_patch_stretches_its_middle_and_keeps_its_corners` |
| `drawPaint` | yes | `draw_paint`, which fills the clip rather than the target — not a rectangle a caller can easily write once a transform is in force | `drawing_the_paint_fills_the_clip_rather_than_the_target` |
| `drawColor` | yes | `draw_color`, which blends and obeys the clip where `clear` replaces and ignores it | `drawing_a_color_blends_where_clearing_replaces` |
| `drawParagraph` | out of scope | shaping and layout are not this project's; `draw_glyphs` takes a positioned run and an atlas | glyph tests |
| `drawVertices` | yes | `draw_vertices`, with positions, texture coordinates and per-vertex colors, and `Paint::with_tint_blend` for how those colors combine | `every_advanced_mode_agrees_with_the_reference_formulas` |
| `drawAtlas`, `drawRawAtlas` | yes | `draw_atlas`, one draw for the whole batch, each sprite with its own transform and color, combined by `Paint::with_tint_blend` | `an_atlas_tints_each_sprite_on_its_own_in_one_draw` |
| `drawPoints`, `drawRawPoints` | yes | `draw_points`, in all three modes. A point is a segment of no length, so the cap is the whole shape | `a_point_is_drawn_as_the_cap_it_would_have_had` |
| `drawDRRect` | yes | `draw_drrect`: two contours filled even-odd, which is what makes the inner one a hole. `draw_drrect_with_radii` takes eight numbers per rectangle, as `drawRRect` does | `the_ring_between_two_rounded_rectangles_is_hollow` |
| `drawShadow` | yes | `draw_shadow`: offset, blur and alpha all from the elevation, under one light | `a_shadow_falls_below_what_casts_it_and_widens_with_elevation` |
| `drawRSuperellipse` | no | | |
| `drawPicture` | yes | `draw_recording`, which composes a finished recording into this one. Tessellated rather than replayed -- see below for what that costs | `a_recording_drawn_into_another_keeps_its_own_layers_and_ramps` |
| `clipRect` | yes | `clip_rect`, and `clip_out_rect` for `ClipOp.difference` | `a_difference_clip_removes_the_rectangle_and_nothing_else` |
| `clipPath` | yes | `clip_path` | `clip-varies-between-draws`, `shape-clipped-fill` |
| `clipRRect` | via | `clip_path` of `Rect::to_rounded_path`, or of `to_rounded_path_with_radii` where the corners differ | |
| `clipRSuperellipse` | no | | |
| `save`, `restore` | yes | `save`, `restore` | `translucent-stack` |
| `saveLayer` | yes | `save_layer`, `save_layer_bounds`, and `save_layer_filtered` for a group filtered as a whole. A `Layer`'s own fields carry a blur, a morphology, a matrix and a color filter in one fixed order; the call takes an `ImageFilter`, which adds a caller's program and a composition in either order | `layer-group-opacity`, `layer-bounded`, `a_group_can_be_filtered_by_a_composition_in_either_order` |
| `pushBackdropFilter` | yes | `save_layer_backdrop` takes any image filter but a matrix, which is refused rather than approximated because it moves the image instead of recomputing it in place; `Layer::with_backdrop_blur` is the blur, which a `Copy` layer can hold. Bounds are the filtered region here rather than an optimization | `layer-backdrop-blurred`, `a_backdrop_takes_any_image_filter_and_refuses_the_one_that_moves_it` |
| `restoreToCount` | via | `save_depth` and a loop | |
| `getSaveCount` | yes | `save_depth` | `saves_nest` |
| `translate`, `scale`, `rotate` | yes | same names | `transformed` |
| `skew` | via | `concat` of the affine | |
| `transform` | yes | `concat_4x4` takes the same column-major 4×4. The reduction to a 3×3 is exact rather than a narrowing: everything drawn here lies on `z = 0`, where a 4×4 never reads its z column. `concat` still takes either, and so does a layer's matrix filter -- which places the finished image rather than the content, the distinction `dart:ui` has both for | `perspective`, `a_gradient_under_perspective_is_locked_to_the_shape` |
| `getTransform` | yes | `transform()` | `save_and_restore_return_the_previous_transform` |
| `getLocalClipBounds`, `getDestinationClipBounds` | yes | `local_clip_bounds` and `destination_clip_bounds`, conservative and accounting for scissor and stencil clips alike | `the_clip_bounds_narrow_with_every_kind_of_clip` |

## Paint

| `dart:ui` | Status | Here | Evidence |
|---|---|---|---|
| `color` | yes | `Paint::fill`. A color states which primaries it is against — sRGB, extended sRGB, or Display P3 — and converts between them; the pipeline works in sRGB primaries with no range limit, carrying components in sRGB's transfer function as upstream does, so a color the triangle cannot describe is carried rather than clipped and reaches a floating-point target intact | `a_color_outside_the_srgb_primaries_reaches_a_floating_point_target` |
| `style` | yes | `with_style` | `rect-fill`, `stroke-polygon-and-curve` |
| `strokeWidth` | yes | `Style::Stroke`, in the space the shape is drawn in. `dart:ui` documents zero as the thinnest line the device can draw; here it draws nothing, so that a caller animating a width down to it stops drawing rather than watching a shape refuse to vanish. A caller wanting a line that stays one pixel wide under zoom divides by the current scale | `stroke-polygon-and-curve`, `rounded-rect-stroked`, `a_shear_is_no_obstacle_to_a_thin_stroke_and_zero_still_means_none` |
| `strokeCap` | yes | `StrokeStyle::cap` | `stroke-caps` |
| `strokeJoin` | yes | `StrokeStyle::join` | `stroke-joins` |
| `strokeMiterLimit` | yes | `StrokeStyle::miter_limit` | `stroke-joins` |
| `isAntiAlias` | yes | `with_anti_alias` | `circle-antialiased`, `curve-antialiased` |
| `blendMode` | yes | `with_blend`, all of Porter-Duff and the fifteen advanced modes where the device offers them. The Porter-Duff modes are weighted sums, so they carry a color outside the sRGB primaries unchanged; the advanced ones are defined by the compositing specification on components between zero and one, so their operands are brought to the triangle's edge first | `advanced-blend-*` |
| `shader` | yes | linear, radial, sweep and conical gradients, images, and a caller's own fragment program | `a_caller_can_fill_a_shape_with_their_own_fragment_program` |
| `colorFilter` | yes | `with_color_filter`: a color matrix, the sRGB transfer function in either direction, and any blend against a constant that is affine in what it blends. Not the advanced blend modes, which the paint's own blend mode covers | `the_gamma_filter_follows_the_curve_at_both_ends_of_it` |
| `imageFilter` | yes | `with_image_filter`: a blur, a matrix, dilate, erode, a color filter, a caller's fragment program and any composition of them, applied to what the paint drew rather than to the color it computed. All seven kinds upstream's `DlImageFilter` offers | `composing_an_erosion_with_a_dilation_depends_on_which_runs_first`, `a_color_filter_can_be_half_of_an_image_filter_composition` |
| `maskFilter` | yes | `with_mask_blur` and `with_mask_blur_style`: a blur of a shape's coverage in all four styles. A shape's blur is stated the way `dart:ui` does it -- the mask blurred, the paint applied through it -- so a gradient or an image blurs correctly. A solid color on a rectangle, rounded rectangle or circle is evaluated in the fragment stage, as upstream's `AttemptDrawBlurredRRect` does it: one draw rather than a layer and two blur passes, in every style. On any other shape a solid color takes the cheaper route of a paint drawn through a blurred layer, which is the same picture only because it does not vary. Solid only over a glyph run, which tints one color however it is drawn | `each_mask_blur_style_keeps_the_part_of_the_blur_it_names` |
| `filterQuality` | yes | `with_sampling`: nearest, linear, the Mitchell bicubic `high` means, and the mip chain `medium` does. An image states whether it carries a chain when it is created, since it costs a third again in memory. The bicubic is the one path that holds a color inside the sRGB primaries: its kernel invents values no texel it read contains, and an overshoot cannot be told from a color outside the triangle at the point it would have to be | `mipmapped_sampling_reads_the_level_built_for_the_size_it_is_drawn_at` |
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

Of forty-eight rows across `Canvas` and `Paint`: forty exist, five are
expressible by a caller who assembles them, two are absent, and one is out of
scope. Counting them is the least interesting thing
about the table -- the absences are not equal, and a reader deciding whether
this renderer is usable should look at which ones rather than how many.

`drawPicture` is now built, and what it turned out to be is worth recording
because the analysis that preceded it was right about the shape and wrong about
the cost. A `Recording` is already tessellated -- paths flattened at a tolerance
taken from the transform in force when they were recorded, vertices and
materials in clip space -- so the flattening cannot be undone and a picture
magnified shows the polygon it became. That much was expected, and it is why
this composes scenes at about the scale they were recorded at rather than being
the reuse optimization the same call is elsewhere.

What was expected to be the work -- carrying clip-space positions and each
material's geometry through the composite affine -- turned out not to be needed
at all. The pass model already had the answer: a layer *is* a pass another pass
samples, so a picture is its passes appended and its root sampled, with nothing
re-recorded and no geometry touched. What is left is arithmetic on indices,
since a picture's layers and baked gradients name positions in lists the
receiving recording is appending to. Getting that wrong makes a picture's layer
sample the host's, which is a plausible picture of something nobody drew, and
there is a test that catches exactly that.

Runtime effects are complete now, and the note that stood here predicted the
wrong obstacle. It said several textures needed a descriptor set of their own.
They did not: a layout may declare bindings a shader never mentions, so widening
the one shared layout to four images serves the solid pipeline unchanged and
gives a caller's program the rest. A second set would have bought an unbounded
count; the ceiling is what buys a single layout, and four is what `dart:ui`
shaders ask for in practice.

That row was described here for a long time as needing a shader pipeline
that compiles at run time. That was wrong too, and the correction was most of
the work: Flutter compiles these ahead of time and ships one payload per
backend, so what was needed was a pipeline cache that can hold more than one
program, not a compiler. `docs/architecture.md` has both designs.

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

`drawVertices` and `drawAtlas` were partial for the same reason and stopped
being so together, which is what the shared mechanism predicted. Both hand the
fragment stage two colors — what the paint produced and what the caller attached
to a vertex or a sprite — and `dart:ui` takes a blend mode saying how to combine
them. All twenty-nine are available, on both of them, through
`Paint::with_tint_blend`.

Every mode is available because neither color is in the framebuffer. The
hardware extensions this renderer uses for `Paint::blend` exist to blend against
a destination a fragment shader cannot read; these two are both already in the
shader, so the whole set is arithmetic there and needs no extension and no
device support. A note here once claimed the opposite, and it made the work
sound architectural when it was a transcription.

The shader's formulas are checked against `impeller_hal`'s, which are the
software reference the conformance tests already compare hardware to. Neither
was derived from the other, so a transcription error shows as a disagreement
rather than as two copies of one mistake.

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
