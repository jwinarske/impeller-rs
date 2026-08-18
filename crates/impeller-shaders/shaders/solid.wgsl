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
