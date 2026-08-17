// Solid colour and linear gradients.
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
// This occupies 112 bytes, within the 128 that every device is required to
// offer. Staying inside the guaranteed minimum matters more here than the
// stop count does — an embedded part that only provides the minimum is exactly
// the hardware this renderer targets.
struct Paint {
    // Up to four stops. Unused entries are ignored rather than blended.
    stops: array<vec4<f32>, 4>,
    // Position of each stop along the gradient, in order.
    offsets: vec4<f32>,
    // Gradient endpoints in clip space: start.xy then end.xy.
    endpoints: vec4<f32>,
    // x: number of stops in use. y: 0 for solid, 1 for a linear gradient.
    params: vec4<f32>,
};

var<push_constant> paint: Paint;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) clip: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.clip = position;
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var colour: vec4<f32> = paint.stops[0];

    if (paint.params.y > 0.5) {
        let start = paint.endpoints.xy;
        let end = paint.endpoints.zw;
        let axis = end - start;
        let length_squared = max(dot(axis, axis), 1e-6);
        // Projection onto the gradient axis, clamped so the ends extend rather
        // than repeat. Tile modes belong to the paint and arrive with them.
        let t = clamp(dot(in.clip - start, axis) / length_squared, 0.0, 1.0);
        colour = sample_stops(t, i32(paint.params.x));
    }

    // Colours are linear here. Conversion to the target's transfer function is
    // the attachment format's job, not this shader's. Premultiplying is this
    // shader's job, because the blend equations expect it.
    return vec4<f32>(colour.rgb * colour.a, colour.a);
}
