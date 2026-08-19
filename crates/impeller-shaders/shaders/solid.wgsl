// Solid color and gradients.
//
// Positions arrive in normalized device coordinates, in the WGSL convention:
// Y increases upward. Translation to a backend whose framebuffer runs the
// other way is naga's job, which is what lets this one source produce matching
// output on every target rather than mirrored output on some of them.
//
// The fragment stage needs to know where it is in order to evaluate a
// gradient, and it learns that from an interpolated clip position rather than
// from the fragment coordinate builtin. That builtin's origin differs between
// the two APIs, so using it would make gradients run in opposite directions on
// each; clip space is normalized by the translator and agrees everywhere.
// Passing it as a varying also avoids a second vertex attribute, which every
// solid draw would otherwise pay for.

// One paint per draw, in a uniform buffer bound at a different offset for each
// draw. It travelled in push constants for a long time, which suited data this
// small and this changeable -- but 128 bytes is all a device guarantees there,
// the material had grown to occupy exactly that, and a material that has to
// sit on top of another one does not fit at any arrangement. A color filter is
// that case.
//
// Every member is a four-component vector on purpose. Under the std140 rules
// this block is declared with, a vec4 and an array of them sit at exactly the
// offsets a flat array of floats would, so the two backends copy the packed
// material in without writing padding around anything.
struct Paint {
    // Up to four stops. Unused entries are ignored rather than blended.
    stops: array<vec4<f32>, 4>,
    // Position of each stop along the gradient, in order.
    offsets: vec4<f32>,
    // Linear: start.xy then end.xy. Radial and sweep: center.xy, then two
    // spare components a sweep uses for its angles.
    geometry: vec4<f32>,
    // Maps a clip-space offset from the center into the gradient's own space,
    // as a two by two matrix in column order.
    //
    // Clip space is anisotropic whenever the target is not square, and a
    // transform may rotate or skew as well, so a circle in user space is an
    // ellipse here. Measuring distance or angle directly in clip space would
    // therefore distort every radial and sweep gradient by the aspect ratio.
    // Mapping back first is what makes them correct under any transform.
    to_local: vec4<f32>,
    // x: number of stops in use. y: 0 solid, 1 linear, 2 radial, 3 sweep.
    params: vec4<f32>,
    // A color filter's matrix, by column: recolor[j] scales the input's jth
    // channel. Named for what it does rather than what it is, because `filter`
    // is a reserved word in this language. Stored by column rather than by row because that makes the
    // application four multiply-adds of vectors, and because a column is a
    // vec4 where a row of the five-wide form a caller writes is not.
    recolor: array<vec4<f32>, 4>,
    // The constant the filter adds.
    filter_offset: vec4<f32>,
    // x: 0 no filter, 1 a matrix on premultiplied color, 2 on straight color.
    filter_params: vec4<f32>,
};

// In its own group so that the texture bindings below, which are rebuilt per
// submission and include a long-lived placeholder set, are untouched by a
// buffer that changes with every draw.
@group(1) @binding(0) var<uniform> paint: Paint;

// One texture per draw. Always bound, even for a paint that does not sample it:
// the alternative is a pipeline variant per material kind, and a binding a
// pipeline declares but no draw supplies is invalid however unreachable the
// branch that would have read it.
@group(0) @binding(0) var image_texture: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) clip: vec2<f32>,
    // Where this vertex reads from a sampled texture. Zero for geometry that
    // samples nothing, which costs an interpolation nobody looks at.
    @location(1) uv: vec2<f32>,
    // A premultiplied color multiplied into whatever the material produced.
    // Opaque white for everything a caller did not color per vertex, and white
    // is the identity.
    @location(2) tint: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.clip = position;
    out.uv = uv;
    out.tint = tint;
    return out;
}

/// Color at `t` along the stop list, with `count` stops in use.
fn sample_stops(t: f32, count: i32) -> vec4<f32> {
    var result: vec4<f32> = paint.stops[0];
    // Walk the stops rather than searching: at most four, and a loop with a
    // data-dependent exit costs more than the comparisons on the hardware this
    // targets.
    for (var i: i32 = 1; i < count; i = i + 1) {
        let lower = paint.offsets[i - 1];
        let upper = paint.offsets[i];
        let span = max(upper - lower, 1e-6);
        let local = clamp((t - lower) / span, 0.0, 1.0);
        // Only segments the parameter has reached contribute, so the last one
        // to apply is the segment it lies in.
        if (t >= lower) {
            result = mix(paint.stops[i - 1], paint.stops[i], local);
        }
    }
    return result;
}

/// Fold a gradient parameter into the ramp according to a tile mode.
///
/// Returns the parameter to sample with in `x`, and in `y` whether the sample
/// counts at all: decal draws nothing outside the ramp, and carrying that as a
/// multiplier keeps a second branch out of every caller.
///
/// The same three codes an image tiles by, because a caller sets both with one
/// `TileMode` and two meanings for one word would be worse than either.
fn tile_gradient(t: f32, tile: f32) -> vec2<f32> {
    if (tile > 0.5 && tile < 1.5) {
        // Repeat. `t - floor(t)` rather than `fract`, which is the same thing
        // for a finite input and says plainly what happens to a negative one:
        // -0.25 lands at 0.75 and the ramp runs on backwards without a seam.
        return vec2<f32>(t - floor(t), 1.0);
    }
    if (tile > 1.5 && tile < 2.5) {
        // Decal. Tested against the parameter as given, since the clamped one
        // is inside by construction.
        let inside = select(0.0, 1.0, t >= 0.0 && t <= 1.0);
        return vec2<f32>(clamp(t, 0.0, 1.0), inside);
    }
    if (tile > 2.5) {
        // Mirror: a period of two, folded in half. The parameter is reduced
        // modulo two and then reflected about one, so 1.25 comes back as 0.75
        // and the ramp runs backwards through the second half of every period.
        // The two ends of a copy meet the two ends of its neighbors, which is
        // the seam that repeating leaves behind.
        return vec2<f32>(1.0 - abs(1.0 - (t - 2.0 * floor(t * 0.5))), 1.0);
    }
    return vec2<f32>(clamp(t, 0.0, 1.0), 1.0);
}

/// The gradient's color at `t`, from wherever this paint keeps its colors.
///
/// Four stops or fewer travel in the material and are walked; more than that
/// were tabulated into a texture by the recorder and are read from it. The two
/// have to agree, so the walk above and the bake that produced the texture
/// implement the same rule, and this is the only place that chooses between
/// them.
///
/// The ramp is sampled at `t` directly rather than at a texel center: the
/// tabulation already placed its samples at centers, so the filter reading
/// between them reconstructs the function rather than shifting it.
///
/// A count of zero is what says the colors were tabulated. It used to be a
/// flag in the fourth parameter slot, which a conical gradient needed for the
/// distance between its two centers -- and a material carrying a ramp has no
/// stop count to report anyway, so the two never wanted separate words.
fn gradient_color(t: f32, count: i32) -> vec4<f32> {
    if (count <= 0) {
        // Straight color, not premultiplied, and decoded from the transfer
        // function by the sampler because the texture is an sRGB format. That
        // is the same shape `sample_stops` returns, so what follows does not
        // need to know which path produced it.
        return textureSampleLevel(image_texture, image_sampler, vec2<f32>(t, 0.5), 0.0);
    }
    return sample_stops(t, count);
}

/// Map a clip-space position into the gradient's own space.
fn to_gradient_space(clip: vec2<f32>) -> vec2<f32> {
    let delta = clip - paint.geometry.xy;
    let column0 = vec2<f32>(paint.to_local.x, paint.to_local.y);
    let column1 = vec2<f32>(paint.to_local.z, paint.to_local.w);
    return column0 * delta.x + column1 * delta.y;
}

/// Sample the bound texture at a clip-space position, premultiplied.
///
/// Unlike every other path here, this returns premultiplied color rather than
/// straight, because that is what it read: render targets store premultiplied
/// and an uploaded image is required to. Scaling the whole vector by the
/// paint's alpha keeps it premultiplied and is exact, where converting to
/// straight alpha and back would divide by an alpha that may be zero and lose
/// precision where it is merely small.
///
/// Getting this wrong is invisible until something samples a translucent
/// texture: with an opaque one the two conventions agree, so a layer nested in
/// another layer is the first thing that shows it.
/// Bring a texture coordinate inside the unit square, the way this paint asks.
///
/// Shared by the two materials that sample a texture at a coordinate -- an
/// image, which computes one from the fragment's position, and a mesh, which
/// is handed one per vertex. Two copies of this would be two chances for a
/// tiled image and a tiled mesh to disagree about what mirroring means.
///
/// Done here rather than through the sampler's address mode so that one
/// sampler serves every draw: the modes are a property of the paint, and
/// baking them into samplers would mean one sampler per combination.
fn tile_uv(uv: vec2<f32>, tile: f32) -> vec2<f32> {
    if (tile > 0.5 && tile < 1.5) {
        // Repeat.
        return fract(uv);
    }
    if (tile > 2.5) {
        // Mirror, per axis: the same fold a gradient uses, applied to each
        // component so a tiled image meets its neighbors on both edges.
        return vec2<f32>(1.0) - abs(vec2<f32>(1.0) - (uv - 2.0 * floor(uv * 0.5)));
    }
    return clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
}

/// A texture read at coordinates the vertices carried, tinted and scaled.
///
/// What a sprite batch draws with. There is no mapping to undo and no source
/// rectangle to apply -- a caller who states a coordinate per vertex has
/// already said which part of the sheet each corner reads -- so this is the
/// image path with everything the vertices already answered taken out.
fn sample_mesh(uv: vec2<f32>) -> vec4<f32> {
    let tile = paint.geometry.y;
    var texel = textureSampleLevel(image_texture, image_sampler, tile_uv(uv, tile), 0.0);
    if (tile > 1.5 && tile < 2.5) {
        // Decal, tested against the coordinate as given, since the tiled one
        // is inside by construction.
        if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
            texel = vec4<f32>(0.0);
        }
    }
    let tint = paint.stops[0];
    let premultiplied = vec4<f32>(tint.rgb * tint.a, tint.a);
    return texel * premultiplied * paint.geometry.x;
}

fn sample_image(clip: vec2<f32>) -> vec4<f32> {
    // The same mapping a radial gradient uses, so an image lands correctly on a
    // target that is not square and under a transform that rotates or scales.
    let uv = to_gradient_space(clip);
    let tile = paint.geometry.w;

    var coord = tile_uv(uv, tile);

    // Into the piece of the texture this paint selected. Applied after the
    // tiling above, so `coord` has already been wrapped within one repetition
    // of the sprite and this maps that repetition onto the sprite's own texels.
    // Mapping first and tiling afterwards would wrap across the whole sheet and
    // draw the neighbors.
    let source = paint.stops[0];
    coord = source.xy + coord * (source.zw - source.xy);

    // Held half a texel inside the selection, because the filter reads two
    // texels and blends them. Without this a sprite shows a seam of whatever
    // sits next to it on the sheet: the edge of a selection falls exactly on a
    // texel boundary, and every sample within half a texel of it mixes in the
    // neighbor. Clamping to texel centers is what makes a selection actually
    // select.
    //
    // For the whole image this changes nothing, because the sampler already
    // clamps to the edge and reading the border texel twice is what it does.
    let half_texel = 0.5 / vec2<f32>(textureDimensions(image_texture));
    let low = min(source.xy + half_texel, source.zw - half_texel);
    let high = max(source.xy + half_texel, source.zw - half_texel);
    coord = clamp(coord, low, high);

    var texel = textureSampleLevel(image_texture, image_sampler, coord, 0.0);
    if (tile > 1.5 && tile < 2.5) {
        // Decal: nothing outside the image's own bounds. Tested against the
        // unclamped coordinate, since the clamped one is inside by
        // construction.
        let outside = any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0));
        if (outside) {
            texel = vec4<f32>(0.0);
        }
    }
    // The tint, stated straight and premultiplied here so it can scale a texel
    // that already is: the color is multiplied by the tint's alpha as well as
    // by its color, which is what keeps the product premultiplied instead of
    // merely close. A white opaque tint is the identity, which is what every
    // image that never mentions one carries.
    let tint = paint.stops[1];
    let premultiplied = vec4<f32>(tint.rgb * tint.a, tint.a);
    return texel * premultiplied * paint.geometry.z;
}

/// Narrow a field to its own outline, where the paint asked for one.
///
/// An outline is the band where the distance is small, so tracing one costs a
/// field nothing that it had not already computed. Tessellating an outline
/// means building a second shape -- offset inward and outward with the corners
/// resolved -- which is where a stroked path gets its vertices and its joins.
///
/// Shared by every shape here so the three cannot drift apart on what a width
/// means: it is the full width, centered on the edge, in the shape's own space.
/// Narrow a field to its own outline, where the paint asked for one.
///
/// An outline is the band where the distance is small, so tracing one costs a
/// field nothing it had not already computed. Tessellating one means building a
/// second shape -- offset inward and outward with the corners resolved -- which
/// is where a stroked path gets its vertices and its joins.
///
/// Both arguments are in the shape's own space, which is where a width is
/// stated. Taking the absolute value here rather than after normalising is
/// deliberate: the gradient is taken of the field before this, because `abs`
/// puts a crease exactly on the curve and a derivative across it would be the
/// one place the edge is measured wrongly.
fn coverage_of(distance: f32, per_pixel: f32, width: f32) -> f32 {
    // Everything inside the edge, which is the whole answer for a fill.
    let inside = clamp(0.5 - distance / per_pixel, 0.0, 1.0);
    if (width <= 0.0) {
        return inside;
    }
    // An outline is a band, and a band is the difference of two half-planes:
    // what is inside its outer edge, less what is inside its inner one. Taking
    // the absolute value of the distance and subtracting half the width says
    // the same thing for a wide band and the wrong thing for a narrow one --
    // once the two ramps overlap, `abs` folds them into a plateau where this
    // subtracts them into the partial coverage they actually make. It also
    // avoids a crease exactly on the curve, which is where two devices'
    // derivative estimates diverge most.
    let outer = clamp(0.5 - (distance - width * 0.5) / per_pixel, 0.0, 1.0);
    let inner = clamp(0.5 - (distance + width * 0.5) / per_pixel, 0.0, 1.0);
    return outer - inner;
}

fn outline_if_asked(distance: f32) -> f32 {
    let width = paint.params.w;
    if (width <= 0.0) {
        return distance;
    }
    return abs(distance) - width * 0.5;
}

/// Signed distance from a rounded rectangle, negative inside.
///
/// The standard formulation: fold into one quadrant by symmetry, measure to
/// the inset box, and subtract the radius. `max(q, 0)` is the distance outside
/// along each axis and `min(max(q.x, q.y), 0)` the distance inside along the
/// nearer one, so the two cases share an expression rather than a branch.
fn rounded_rect_distance(point: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(point) - half_size + vec2<f32>(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - radius;
}

/// A rounded rectangle evaluated here rather than built out of triangles.
///
/// Two triangles cover it whatever the radius, where tessellating costs
/// vertices in proportion to how round it is. The edge antialiases from the
/// distance the shape already computes, which is a better edge than
/// multisampling gives and costs one sample rather than four.
fn rounded_rect_coverage(clip: vec2<f32>) -> vec4<f32> {
    let point = to_gradient_space(clip);
    let half_size = paint.geometry.zw;
    let radius = clamp(paint.params.z, 0.0, min(half_size.x, half_size.y));
    let distance = rounded_rect_distance(point, half_size, radius);

    // How much the distance changes across one pixel, which is what turns a
    // distance into a coverage. Taken from the derivative rather than passed
    // in, so it stays right under a transform the paint never sees -- a shape
    // scaled up antialiases over the same pixel, not the same unit.
    //
    // The gradient's magnitude, not `fwidth`. `fwidth` sums the two partial
    // derivatives, which is their L1 norm and over-estimates the true rate by
    // up to the square root of two -- and by much more under a transform that
    // scales the axes differently, where it adds a large derivative to a small
    // one. The result is an edge softer than a pixel, which reads as a blurry
    // shape rather than an antialiased one.
    let gradient = vec2<f32>(dpdx(distance), dpdy(distance));
    let width = length(gradient);
    // Half a pixel each way. A pixel whose center sits on the edge is half
    // covered, which is what the linear ramp says at distance zero.
    let coverage = coverage_of(distance, max(width, 1e-6), paint.params.w);

    let tint = paint.stops[0];
    let alpha = tint.a * coverage;
    return vec4<f32>(tint.rgb * alpha, alpha);
}

/// An ellipse evaluated here, with coverage from an approximate distance.
///
/// There is no closed form for the exact distance to an ellipse. There is one
/// for a good approximation of it: the implicit function divided by the length
/// of its own gradient, which is the first-order estimate of how far away the
/// curve is. It costs two lengths and a divide, no iteration, and it is nearly
/// exact where it matters -- within a pixel of the boundary, which is the only
/// place coverage is between nothing and all of it. Measured against a densely
/// sampled true distance it is within a thousandth of a unit there, and drifts
/// only well inside, where coverage has saturated and nothing can see it.
fn ellipse_coverage(clip: vec2<f32>) -> vec4<f32> {
    let point = to_gradient_space(clip);
    let axes = max(paint.geometry.zw, vec2<f32>(1e-6));

    // The implicit function itself: zero on the curve, negative inside and
    // positive outside, in no particular units.
    let implicit = length(point / axes) - 1.0;
    // How fast it changes across a pixel, which is what turns it into pixels
    // without ever forming a distance. Taken before the outline, since `abs`
    // creases on the curve.
    let gradient = vec2<f32>(dpdx(implicit), dpdy(implicit));
    let per_pixel = max(length(gradient), 1e-6);

    var stroke = paint.params.w;
    if (stroke > 0.0) {
        // A width is stated in the shape's own space and this function is in
        // none, so it is converted by how fast the function changes per unit
        // there -- which for this one is the ratio of the two lengths below.
        let k1 = max(length(point / axes), 1e-6);
        let k2 = length(point / (axes * axes));
        stroke = stroke * k2 / k1;
    }
    let coverage = coverage_of(implicit, per_pixel, stroke);
    let tint = paint.stops[0];
    let alpha = tint.a * coverage;
    return vec4<f32>(tint.rgb * alpha, alpha);
}

/// One axis of a separable Gaussian blur of the bound texture.
///
/// Two passes of this give the same result as a square of taps, because a
/// two-dimensional Gaussian is the product of two one-dimensional ones -- at a
/// radius of sixteen, thirty-three taps against a thousand and eighty-nine.
///
/// The weights are computed rather than looked up. A table would need a size
/// chosen in advance and a branch per size; an exponential per tap is a handful
/// of instructions on hardware that has one, and this is bandwidth-bound long
/// before it is arithmetic-bound.
fn blur_along_axis(clip: vec2<f32>) -> vec4<f32> {
    let uv = to_gradient_space(clip);
    let step = paint.geometry.zw;
    let sigma = max(paint.params.z, 1e-4);
    // Three deviations each way covers better than four nines of the curve;
    // past that a tap contributes less than an eight-bit target can represent.
    let reach = sigma * 3.0;
    // A shader loop must be bounded, and sixty-five taps is already a lot of
    // bandwidth per pixel per pass.
    let max_taps = 32.0;
    // One tap per texel while that fits within the budget, and further apart
    // once it does not. Spreading rather than truncating is what keeps a large
    // blur soft: cut off at the budget, the taps stop covering the curve and
    // the result stops getting softer however large sigma grows -- the shape
    // creeps toward a box the wider it is asked to be. Spread, the taps still
    // span three deviations and the sampler's bilinear filter averages the
    // texels each one falls between, which is what makes the gaps cost quality
    // rather than correctness.
    let spread = max(reach / max_taps, 1.0);
    let taps = min(ceil(reach / spread), max_taps);

    // Hoisted out of the loop, which the compiler may or may not do for a
    // divide by a uniform expression.
    let denominator = -0.5 / (sigma * sigma);
    var total = vec4<f32>(0.0);
    var weight_sum = 0.0;
    var i = -taps;
    loop {
        if (i > taps) { break; }
        // Weighted by where the tap actually lands, not by which tap it is.
        let offset = i * spread;
        let weight = exp(offset * offset * denominator);
        // Clamped, so the edge extends rather than the blur pulling in
        // transparent black from outside and darkening the border. The target
        // is sized to the content, so there is nothing outside worth reading.
        let coord = clamp(uv + step * offset, vec2<f32>(0.0), vec2<f32>(1.0));
        total = total + textureSampleLevel(image_texture, image_sampler, coord, 0.0) * weight;
        weight_sum = weight_sum + weight;
        i = i + 1.0;
    }
    // Normalized by what was actually summed rather than by the analytic
    // integral, so a truncated tail does not darken the result.
    return total / max(weight_sum, 1e-6);
}

/// Apply this paint's color filter, if it has one.
///
/// Takes and returns premultiplied color, because that is what every path
/// through `shade` produces and what the blend equations expect. A filter
/// stated on straight color -- which is how a color matrix is written, and how
/// `dart:ui` defines one -- has the alpha divided out first and put back after.
///
/// Only affine filters exist here, so this is one matrix and no branching on
/// what the filter means. What that costs is that the blend modes offered as
/// filters are the ones affine in their other operand; deciding which those
/// are happens on the processor, where it is a table rather than a branch per
/// fragment.
fn filtered(premultiplied: vec4<f32>) -> vec4<f32> {
    let kind = paint.filter_params.x;
    if (kind < 0.5) {
        return premultiplied;
    }

    let straight = kind > 1.5;
    var color = premultiplied;
    if (straight) {
        // A fully transparent pixel has no color to recover -- every channel
        // is zero whatever it was -- so the divisor is floored rather than
        // guarded, which gives zero and is the right answer.
        let alpha = max(color.a, 1e-6);
        color = vec4<f32>(color.rgb / alpha, color.a);
    }

    var out = paint.recolor[0] * color.r
        + paint.recolor[1] * color.g
        + paint.recolor[2] * color.b
        + paint.recolor[3] * color.a
        + paint.filter_offset;

    if (straight) {
        out = clamp(out, vec4<f32>(0.0), vec4<f32>(1.0));
        return vec4<f32>(out.rgb * out.a, out.a);
    }
    // Premultiplied already, so the invariant to keep is the one that form
    // carries: no channel may exceed the alpha it was multiplied by. A filter
    // is free to produce a color that does, and a color brighter than its own
    // alpha is not a color -- it composites as though it were lit from
    // nowhere.
    let alpha = clamp(out.a, 0.0, 1.0);
    return vec4<f32>(clamp(out.rgb, vec3<f32>(0.0), vec3<f32>(alpha)), alpha);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The vertex color multiplies the material, and the filter applies to what
    // that produced -- which is the order `dart:ui` states: a color filter acts
    // on the paint's result, and a mesh's own colors are part of producing it.
    //
    // Both sides are premultiplied, so this is a componentwise product and
    // stays premultiplied. Multiplying by a straight color instead would leave
    // the alpha applied once to the color and twice to itself.
    return filtered(shade(in) * in.tint);
}

/// The color this paint produces, premultiplied, before any filter.
fn shade(in: VertexOutput) -> vec4<f32> {
    var color: vec4<f32> = paint.stops[0];
    let kind = paint.params.y;
    let count = i32(paint.params.x);

    if (kind > 0.5 && kind < 1.5) {
        // Linear: project onto the axis in the gradient's own space rather than
        // in clip space. Clip space is normalized to the target, so projecting
        // there weights the two axes by the target's shape and runs a diagonal
        // gradient in the wrong direction on anything that is not square.
        //
        // The parameter is taken unclamped and folded afterwards, because what
        // happens past either end is the paint's to say.
        let axis = paint.geometry.zw;
        let length_squared = max(dot(axis, axis), 1e-6);
        let t = dot(to_gradient_space(in.clip), axis) / length_squared;
        let tiled = tile_gradient(t, paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y;
    } else if (kind > 1.5 && kind < 2.5) {
        // Radial: distance in gradient space, where the radius is one.
        let tiled = tile_gradient(length(to_gradient_space(in.clip)), paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y;
    } else if (kind > 2.5 && kind < 3.5) {
        // Sweep: angle about the center, measured in gradient space so an
        // anisotropic target does not bunch the stops on two sides.
        let local = to_gradient_space(in.clip);
        let angle = atan2(local.y, local.x);
        let start_angle = paint.geometry.z;
        let sweep = max(paint.geometry.w - start_angle, 1e-6);
        // The angle is brought into one turn ahead of the start, which removes
        // the atan2 branch cut: that discontinuity is a property of how the
        // angle was computed and not of the gradient, so a sweep beginning at
        // any angle runs forward from there without a seam.
        //
        // Dividing by a full turn rather than by the sweep is the distinction
        // that makes a tile mode possible at all. Folding by the sweep would
        // send every direction back inside the arc, which *is* repeating, and
        // would leave clamp and decal with nothing outside to act on. Past the
        // end of a partial sweep the parameter now exceeds one, and what
        // happens there is the paint's to say.
        let delta = angle - start_angle;
        let turn = 6.28318530717958647692;
        let ahead = delta - turn * floor(delta / turn);
        let tiled = tile_gradient(ahead / sweep, paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y;
    } else if (kind > 8.5 && kind < 9.5) {
        // Conical: the parameter is where a point sits between two circles,
        // which is the general gradient the other two are special cases of. A
        // point lies on the circle interpolated at `t` when
        //
        //     |p - t*d| = r0 + t*dr
        //
        // and squaring that is a quadratic in `t`, which is what this solves.
        // The mapping put the first center at the origin and the second on the
        // positive x axis, so `d` is `(separation, 0)` and the dot products
        // below lose a term rather than needing one.
        let point = to_gradient_space(in.clip);
        let separation = paint.params.w;
        let r0 = paint.geometry.z;
        let dr = paint.geometry.w;

        let a = separation * separation - dr * dr;
        let b = point.x * separation + r0 * dr;
        let c = dot(point, point) - r0 * r0;

        // Relative, because everything here is in the gradient's own units and
        // a fixed threshold would call a small gradient degenerate and a large
        // one well behaved for the same shape.
        let magnitude = max(separation * separation, dr * dr);

        var t = 0.0;
        var covered = false;
        if (abs(a) > magnitude * 1e-5) {
            let disc = b * b - a * c;
            if (disc >= 0.0) {
                let root = sqrt(disc);
                // Both roots are circles through the point; the larger `t` is
                // the one nearer the second circle and is what the gradient
                // means. A circle of negative radius is not one, so a root
                // producing one is rejected before the other is tried.
                let far = max((b + root) / a, (b - root) / a);
                let near = min((b + root) / a, (b - root) / a);
                if (r0 + far * dr >= 0.0) {
                    t = far;
                    covered = true;
                } else if (r0 + near * dr >= 0.0) {
                    t = near;
                    covered = true;
                }
            }
        } else if (abs(b) > 1e-6) {
            // The quadratic term vanishes when the two circles are tangent
            // internally -- the cone becomes a half-plane -- and what is left
            // is linear. Solving the quadratic here would divide by zero and
            // lose the one root that exists.
            let only = c / (2.0 * b);
            if (r0 + only * dr >= 0.0) {
                t = only;
                covered = true;
            }
        }

        // Outside the cone there is no circle through the point at all, and no
        // tile mode changes that: repeating a parameter that does not exist
        // would invent one. Drawing nothing is what the two circles say.
        let tiled = tile_gradient(t, paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y * select(0.0, 1.0, covered);
    }
    // Checked after the gradient chain rather than inside it, because the
    // sweep arm tests only a lower bound and would otherwise claim this kind
    // as well. Returned directly, since a sampled texel is premultiplied
    // already and the conversion below would apply alpha a second time.
    if (kind > 3.5 && kind < 4.5) {
        return sample_image(in.clip);
    }
    // Every one of these is bounded on both sides, so each arm claims its own
    // kind and nothing else. Written as descending open-ended tests they were
    // correct only because of the returns above them, which meant a kind added
    // later was claimed by all four: the conical gradient, kind nine, came out
    // as a rounded rectangle the first time it ran.
    if (kind > 7.5 && kind < 8.5) {
        return ellipse_coverage(in.clip);
    }
    if (kind > 6.5 && kind < 7.5) {
        return rounded_rect_coverage(in.clip);
    }
    if (kind > 5.5 && kind < 6.5) {
        // Already premultiplied, like anything else sampled from a target, and
        // a weighted average of premultiplied colors is premultiplied.
        return blur_along_axis(in.clip);
    }
    if (kind > 9.5 && kind < 10.5) {
        // Already premultiplied, like anything else sampled from a texture.
        return sample_mesh(in.uv);
    }
    if (kind > 4.5 && kind < 5.5) {
        // Coverage rather than color: one channel scaling a solid, which is
        // what an antialiased glyph is. The coordinates are the vertex's own,
        // so a run of glyphs reading different parts of one atlas needs one
        // draw and one paint between them.
        //
        // Read from the red channel, which is where a single-channel atlas
        // puts it and where a four-channel one repeats it.
        let coverage = textureSampleLevel(image_texture, image_sampler, in.uv, 0.0).r;
        let tint = paint.stops[0];
        let alpha = tint.a * coverage;
        return vec4<f32>(tint.rgb * alpha, alpha);
    }

    // Colors are linear here. Conversion to the target's transfer function is
    // the attachment format's job, not this shader's. Premultiplying is this
    // shader's job, because the blend equations expect it.
    return vec4<f32>(color.rgb * color.a, color.a);
}
