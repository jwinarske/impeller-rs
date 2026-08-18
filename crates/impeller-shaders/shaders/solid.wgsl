// Solid colour and gradients.
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

// One paint per draw, in push constants: the data is small and changes every
// draw, so a uniform buffer would need either a fresh allocation or a dynamic
// offset each time.
//
// This occupies 128 bytes, which is exactly what every device is required to
// offer and therefore the ceiling.
//
// The image shader was expected to be what broke that budget, and it is not:
// its mapping reuses the same origin-plus-matrix pair a radial gradient needs,
// and the texture it samples is a binding rather than data, so it costs no
// push-constant space at all. What would break the budget is a material
// wanting both a gradient's stops and an image's mapping at once. Nothing does
// yet, and that is the point at which a uniform buffer becomes the answer.
struct Paint {
    // Up to four stops. Unused entries are ignored rather than blended.
    stops: array<vec4<f32>, 4>,
    // Position of each stop along the gradient, in order.
    offsets: vec4<f32>,
    // Linear: start.xy then end.xy. Radial and sweep: centre.xy, then two
    // spare components a sweep uses for its angles.
    geometry: vec4<f32>,
    // Maps a clip-space offset from the centre into the gradient's own space,
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
};

var<push_constant> paint: Paint;

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
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.clip = position;
    out.uv = uv;
    return out;
}

/// Colour at `t` along the stop list, with `count` stops in use.
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
fn sample_image(clip: vec2<f32>) -> vec4<f32> {
    // The same mapping a radial gradient uses, so an image lands correctly on a
    // target that is not square and under a transform that rotates or scales.
    let uv = to_gradient_space(clip);
    let tile = paint.geometry.w;

    var coord = uv;
    if (tile > 0.5 && tile < 1.5) {
        // Repeat. Done here rather than through the sampler's address mode so
        // that one sampler serves every draw; the modes are a property of the
        // paint, and baking them into samplers would mean one per combination.
        coord = fract(uv);
    } else {
        coord = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    }

    var texel = textureSampleLevel(image_texture, image_sampler, coord, 0.0);
    if (tile > 1.5) {
        // Decal: nothing outside the image's own bounds. Tested against the
        // unclamped coordinate, since the clamped one is inside by
        // construction.
        let outside = any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0));
        if (outside) {
            texel = vec4<f32>(0.0);
        }
    }
    return texel * paint.geometry.z;
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
    // Half a pixel each way. A pixel whose centre sits on the edge is half
    // covered, which is what the linear ramp says at distance zero.
    let coverage = clamp(0.5 - distance / max(width, 1e-6), 0.0, 1.0);

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

    // The implicit function itself, which is zero on the curve, negative
    // inside and positive outside -- but in no particular units.
    let implicit = length(point / axes) - 1.0;
    // Divided by how fast it changes across a pixel, which converts it to
    // pixels without ever forming a distance. Normalising twice -- once into
    // local units by the field's own gradient, then again by the screen
    // derivative -- is what the longer formulation does, and each step is an
    // approximation whose error two devices need not share.
    let gradient = vec2<f32>(dpdx(implicit), dpdy(implicit));
    let coverage = clamp(0.5 - implicit / max(length(gradient), 1e-6), 0.0, 1.0);
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var colour: vec4<f32> = paint.stops[0];
    let kind = paint.params.y;
    let count = i32(paint.params.x);

    if (kind > 0.5 && kind < 1.5) {
        // Linear: project onto the axis in the gradient's own space rather than
        // in clip space. Clip space is normalized to the target, so projecting
        // there weights the two axes by the target's shape and runs a diagonal
        // gradient in the wrong direction on anything that is not square.
        //
        // Clamped so the ends extend rather than repeat; tile modes arrive with
        // the paint.
        let axis = paint.geometry.zw;
        let length_squared = max(dot(axis, axis), 1e-6);
        let t = clamp(dot(to_gradient_space(in.clip), axis) / length_squared, 0.0, 1.0);
        colour = sample_stops(t, count);
    } else if (kind > 1.5 && kind < 2.5) {
        // Radial: distance in gradient space, where the radius is one.
        let t = clamp(length(to_gradient_space(in.clip)), 0.0, 1.0);
        colour = sample_stops(t, count);
    } else if (kind > 2.5 && kind < 3.5) {
        // Sweep: angle about the centre, measured in gradient space so an
        // anisotropic target does not bunch the stops on two sides.
        let local = to_gradient_space(in.clip);
        let angle = atan2(local.y, local.x);
        let start_angle = paint.geometry.z;
        let sweep = max(paint.geometry.w - start_angle, 1e-6);
        // Wrapped into a single turn so a sweep starting at any angle runs
        // forward from there rather than clipping at the atan2 discontinuity.
        var turns = (angle - start_angle) / sweep;
        turns = turns - floor(turns);
        colour = sample_stops(clamp(turns, 0.0, 1.0), count);
    }
    // Checked after the gradient chain rather than inside it, because the
    // sweep arm tests only a lower bound and would otherwise claim this kind
    // as well. Returned directly, since a sampled texel is premultiplied
    // already and the conversion below would apply alpha a second time.
    if (kind > 3.5 && kind < 4.5) {
        return sample_image(in.clip);
    }
    if (kind > 7.5) {
        return ellipse_coverage(in.clip);
    }
    if (kind > 6.5) {
        return rounded_rect_coverage(in.clip);
    }
    if (kind > 5.5) {
        // Already premultiplied, like anything else sampled from a target, and
        // a weighted average of premultiplied colors is premultiplied.
        return blur_along_axis(in.clip);
    }
    if (kind > 4.5) {
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

    // Colours are linear here. Conversion to the target's transfer function is
    // the attachment format's job, not this shader's. Premultiplying is this
    // shader's job, because the blend equations expect it.
    return vec4<f32>(colour.rgb * colour.a, colour.a);
}
