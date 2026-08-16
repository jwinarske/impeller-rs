// Solid-color fill: the simplest material, and the one every other material
// is validated against.
//
// Positions arrive in normalized device coordinates, in the WGSL convention:
// Y increases upward. Translation to a backend whose framebuffer runs the
// other way is naga's job, which is what lets this one source produce matching
// output on every target rather than mirrored output on some of them.
//
// Placing the transform in the vertex shader comes with the renderer, which
// owns the transform stack; baking one in here would mean two places decide
// where geometry lands.

// One paint per draw. Push constants rather than a uniform buffer because the
// data is small and changes every draw: a buffer would need either a fresh
// allocation or a dynamic offset per draw, and both cost more than the handful
// of bytes this occupies.
struct Paint {
    color: vec4<f32>,
};

var<push_constant> paint: Paint;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Colors are linear here. Conversion to the target's transfer function is
    // the attachment format's job, not this shader's.
    return paint.color;
}
