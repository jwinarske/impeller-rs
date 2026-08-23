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
    vec3 clip;
    vec2 uv;
    vec4 tint;
};
layout(location = 0) in vec3 _p2vs_location0;
layout(location = 1) in vec2 _p2vs_location1;
layout(location = 2) in vec4 _p2vs_location2;
smooth out vec3 _vs2fs_location0;
smooth out vec2 _vs2fs_location1;
smooth out vec4 _vs2fs_location2;

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

float cubic_weight(float x) {
    float t_4 = abs(x);
    if ((t_4 < 1.0)) {
        float cubic_1 = ((12.0 - (9.0 * 0.33333334)) - (6.0 * 0.33333334));
        float square = ((-18.0 + (12.0 * 0.33333334)) + (6.0 * 0.33333334));
        return ((((((cubic_1 * t_4) + square) * t_4) * t_4) + (6.0 - (2.0 * 0.33333334))) / 6.0);
    }
    if ((t_4 < 2.0)) {
        float cubic_2 = (-(0.33333334) - (6.0 * 0.33333334));
        float square_1 = ((6.0 * 0.33333334) + (30.0 * 0.33333334));
        float linear = ((-12.0 * 0.33333334) - (48.0 * 0.33333334));
        return (((((((cubic_2 * t_4) + square_1) * t_4) + linear) * t_4) + ((8.0 * 0.33333334) + (24.0 * 0.33333334))) / 6.0);
    }
    return 0.0;
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

vec3 linear_to_srgb(vec3 c) {
    vec3 low_2 = (c * 12.92);
    vec3 high_2 = ((1.055 * pow(max(c, vec3(0.0)), vec3(0.41666666))) - vec3(0.055));
    return mix(high_2, low_2, lessThanEqual(c, vec3(0.0031308)));
}

vec3 srgb_to_linear(vec3 c_1) {
    vec3 low_3 = (c_1 / vec3(12.92));
    vec3 high_3 = pow(((max(c_1, vec3(0.0)) + vec3(0.055)) / vec3(1.055)), vec3(2.4));
    return mix(high_3, low_3, lessThanEqual(c_1, vec3(0.04045)));
}

float hard_light(float cb, float cs) {
    if ((cs <= 0.5)) {
        return (cb * (2.0 * cs));
    }
    float d = ((2.0 * cs) - 1.0);
    return ((cb + d) - (cb * d));
}

float separable_b(int mode, float cb_1, float cs_1) {
    switch(mode) {
        case 14: {
            return (cb_1 * cs_1);
        }
        case 15: {
            return ((cb_1 + cs_1) - (cb_1 * cs_1));
        }
        case 16: {
            float _e7 = hard_light(cs_1, cb_1);
            return _e7;
        }
        case 17: {
            return min(cb_1, cs_1);
        }
        case 18: {
            return max(cb_1, cs_1);
        }
        case 19: {
            if ((cb_1 <= 0.0)) {
                return 0.0;
            }
            if ((cs_1 >= 1.0)) {
                return 1.0;
            }
            return min((cb_1 / (1.0 - cs_1)), 1.0);
        }
        case 20: {
            if ((cb_1 >= 1.0)) {
                return 1.0;
            }
            if ((cs_1 <= 0.0)) {
                return 0.0;
            }
            return (1.0 - min(((1.0 - cb_1) / cs_1), 1.0));
        }
        case 21: {
            float _e34 = hard_light(cb_1, cs_1);
            return _e34;
        }
        case 22: {
            float d_1 = ((cb_1 > 0.25) ? inversesqrt(max(cb_1, 1e-8)) : ((((16.0 * cb_1) - 12.0) * cb_1) + 4.0));
            float dd = ((cb_1 > 0.25) ? sqrt(max(cb_1, 0.0)) : (d_1 * cb_1));
            if ((cs_1 <= 0.5)) {
                return (cb_1 - (((1.0 - (2.0 * cs_1)) * cb_1) * (1.0 - cb_1)));
            }
            return (cb_1 + (((2.0 * cs_1) - 1.0) * (dd - cb_1)));
        }
        case 23: {
            return abs((cb_1 - cs_1));
        }
        case 24: {
            return ((cb_1 + cs_1) - ((2.0 * cb_1) * cs_1));
        }
        default: {
            return cs_1;
        }
    }
}

float lum(vec3 c_2) {
    return dot(c_2, vec3(0.3, 0.59, 0.11));
}

vec3 set_lum(vec3 c_3, float l) {
    vec3 out_2 = vec3(0.0);
    float _e2 = lum(c_3);
    vec3 shifted = (c_3 + vec3((l - _e2)));
    float low_4 = min(shifted.x, min(shifted.y, shifted.z));
    float high_4 = max(shifted.x, max(shifted.y, shifted.z));
    float _e16 = lum(shifted);
    out_2 = shifted;
    if ((low_4 < 0.0)) {
        vec3 _e20 = out_2;
        out_2 = (vec3(_e16) + (((_e20 - vec3(_e16)) * _e16) / vec3(max((_e16 - low_4), 1e-8))));
    }
    if ((high_4 > 1.0)) {
        vec3 _e33 = out_2;
        out_2 = (vec3(_e16) + (((_e33 - vec3(_e16)) * (1.0 - _e16)) / vec3(max((high_4 - _e16), 1e-8))));
    }
    vec3 _e46 = out_2;
    return _e46;
}

float sat(vec3 c_4) {
    return (max(c_4.x, max(c_4.y, c_4.z)) - min(c_4.x, min(c_4.y, c_4.z)));
}

vec3 set_sat(vec3 c_5, float s) {
    float low_5 = min(c_5.x, min(c_5.y, c_5.z));
    float high_5 = max(c_5.x, max(c_5.y, c_5.z));
    if ((high_5 <= low_5)) {
        return vec3(0.0);
    }
    return (((c_5 - vec3(low_5)) * s) / vec3((high_5 - low_5)));
}

vec3 nonseparable_b(int mode_1, vec3 cb_2, vec3 cs_2) {
    switch(mode_1) {
        case 25: {
            float _e3 = sat(cb_2);
            vec3 _e4 = set_sat(cs_2, _e3);
            float _e5 = lum(cb_2);
            vec3 _e6 = set_lum(_e4, _e5);
            return _e6;
        }
        case 26: {
            float _e7 = sat(cs_2);
            vec3 _e8 = set_sat(cb_2, _e7);
            float _e9 = lum(cb_2);
            vec3 _e10 = set_lum(_e8, _e9);
            return _e10;
        }
        case 27: {
            float _e11 = lum(cb_2);
            vec3 _e12 = set_lum(cs_2, _e11);
            return _e12;
        }
        case 28: {
            float _e13 = lum(cs_2);
            vec3 _e14 = set_lum(cb_2, _e13);
            return _e14;
        }
        default: {
            return cs_2;
        }
    }
}

vec4 blend_tint(int mode_2, vec4 src, vec4 dst) {
    vec3 mixed = vec3(0.0);
    float sa = src.w;
    float da = dst.w;
    switch(mode_2) {
        case 0: {
            return vec4(0.0);
        }
        case 1: {
            return src;
        }
        case 2: {
            return dst;
        }
        case 3: {
            return (src + (dst * (1.0 - sa)));
        }
        case 4: {
            return (dst + (src * (1.0 - da)));
        }
        case 5: {
            return (src * da);
        }
        case 6: {
            return (dst * sa);
        }
        case 7: {
            return (src * (1.0 - da));
        }
        case 8: {
            return (dst * (1.0 - sa));
        }
        case 9: {
            return ((src * da) + (dst * (1.0 - sa)));
        }
        case 10: {
            return ((dst * sa) + (src * (1.0 - da)));
        }
        case 11: {
            return ((src * (1.0 - da)) + (dst * (1.0 - sa)));
        }
        case 12: {
            return min((src + dst), vec4(1.0));
        }
        case 13: {
            return (src * dst);
        }
        default: {
            break;
        }
    }
    vec3 cs_3 = ((sa <= 0.0) ? vec3(0.0) : (src.xyz / vec3(sa)));
    vec3 cb_3 = ((da <= 0.0) ? vec3(0.0) : (dst.xyz / vec3(da)));
    if ((mode_2 >= 25)) {
        vec3 _e64 = nonseparable_b(mode_2, cb_3, cs_3);
        mixed = _e64;
    } else {
        float _e67 = separable_b(mode_2, cb_3.x, cs_3.x);
        float _e70 = separable_b(mode_2, cb_3.y, cs_3.y);
        float _e73 = separable_b(mode_2, cb_3.z, cs_3.z);
        mixed = vec3(_e67, _e70, _e73);
    }
    vec3 _e80 = mixed;
    vec3 rgb = ((((sa * (1.0 - da)) * cs_3) + ((sa * da) * _e80)) + (((1.0 - sa) * da) * cb_3));
    return vec4(rgb, (sa + (da * (1.0 - sa))));
}

void main() {
    vec3 position = _p2vs_location0;
    vec2 uv = _p2vs_location1;
    vec4 tint = _p2vs_location2;
    VertexOutput out_ = VertexOutput(vec4(0.0), vec3(0.0), vec2(0.0), vec4(0.0));
    out_.position = vec4(position.xy, 0.0, position.z);
    out_.clip = position;
    out_.uv = uv;
    out_.tint = tint;
    VertexOutput _e12 = out_;
    gl_Position = _e12.position;
    _vs2fs_location0 = _e12.clip;
    _vs2fs_location1 = _e12.uv;
    _vs2fs_location2 = _e12.tint;
    gl_Position.yz = vec2(-gl_Position.y, gl_Position.z * 2.0 - gl_Position.w);
    return;
}

