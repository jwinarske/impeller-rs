#version 300 es

precision highp float;
precision highp int;

struct Paint {
    vec4 stops[4];
    vec4 offsets;
    vec4 geometry;
    vec4 to_local;
    vec4 params;
    vec4 recolor[4];
    vec4 filter_offset;
    vec4 filter_params;
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

vec2 tile_gradient(float t_1, float tile) {
    if (((tile > 0.5) && (tile < 1.5))) {
        return vec2((t_1 - floor(t_1)), 1.0);
    }
    if (((tile > 1.5) && (tile < 2.5))) {
        float inside = (((t_1 >= 0.0) && (t_1 <= 1.0)) ? 1.0 : 0.0);
        return vec2(clamp(t_1, 0.0, 1.0), inside);
    }
    if ((tile > 2.5)) {
        return vec2((1.0 - abs((1.0 - (t_1 - (2.0 * floor((t_1 * 0.5))))))), 1.0);
    }
    return vec2(clamp(t_1, 0.0, 1.0), 1.0);
}

vec2 tile_uv(vec2 uv_1, float tile_1) {
    if (((tile_1 > 0.5) && (tile_1 < 1.5))) {
        return fract(uv_1);
    }
    if ((tile_1 > 2.5)) {
        return (vec2(1.0) - abs((vec2(1.0) - (uv_1 - (2.0 * floor((uv_1 * 0.5)))))));
    }
    return clamp(uv_1, vec2(0.0), vec2(1.0));
}

float coverage_of(float distance_, float per_pixel, float width) {
    float inside_1 = clamp((0.5 - (distance_ / per_pixel)), 0.0, 1.0);
    if ((width <= 0.0)) {
        return inside_1;
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

