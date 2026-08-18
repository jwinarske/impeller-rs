#version 300 es

precision highp float;
precision highp int;

struct Paint {
    vec4 stops[4];
    vec4 offsets;
    vec4 geometry;
    vec4 to_local;
    vec4 params;
};
struct VertexOutput {
    vec4 position;
    vec2 clip;
    vec2 uv;
};
layout(location = 0) in vec2 _p2vs_location0;
layout(location = 1) in vec2 _p2vs_location1;
smooth out vec2 _vs2fs_location0;
smooth out vec2 _vs2fs_location1;

float coverage_of(float distance_, float per_pixel, float width) {
    float inside = clamp((0.5 - (distance_ / per_pixel)), 0.0, 1.0);
    if ((width <= 0.0)) {
        return inside;
    }
    float outer = clamp((0.5 - ((distance_ - (width * 0.5)) / per_pixel)), 0.0, 1.0);
    float inner = clamp((0.5 - ((distance_ + (width * 0.5)) / per_pixel)), 0.0, 1.0);
    return (outer - inner);
}

float rounded_rect_distance(vec2 point, vec2 half_size, float radius) {
    vec2 q = ((abs(point) - half_size) + vec2(radius));
    return ((min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0)))) - radius);
}

void main() {
    vec2 position = _p2vs_location0;
    vec2 uv = _p2vs_location1;
    VertexOutput out_ = VertexOutput(vec4(0.0), vec2(0.0), vec2(0.0));
    out_.position = vec4(position, 0.0, 1.0);
    out_.clip = position;
    out_.uv = uv;
    VertexOutput _e9 = out_;
    gl_Position = _e9.position;
    _vs2fs_location0 = _e9.clip;
    _vs2fs_location1 = _e9.uv;
    gl_Position.yz = vec2(-gl_Position.y, gl_Position.z * 2.0 - gl_Position.w);
    return;
}

