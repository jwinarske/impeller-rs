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
    vec4 tint;
};
layout(std140) uniform Paint_block_0Fragment { Paint _group_1_binding_0_fs; };

uniform highp sampler2D _group_0_binding_0_fs;

smooth in vec2 _vs2fs_location0;
smooth in vec2 _vs2fs_location1;
smooth in vec4 _vs2fs_location2;
layout(location = 0) out vec4 _fs2p_location0;

vec4 sample_stops(float t, int count) {
    vec4 result = vec4(0.0);
    int i = 1;
    vec4 _e5 = _group_1_binding_0_fs.stops[0];
    result = _e5;
    bool loop_init = true;
    while(true) {
        if (!loop_init) {
            int _e45 = i;
            i = (_e45 + 1);
        }
        loop_init = false;
        int _e9 = i;
        if ((_e9 < count)) {
        } else {
            break;
        }
        {
            int _e13 = i;
            float lower = _group_1_binding_0_fs.offsets[(_e13 - 1)];
            int _e20 = i;
            float upper = _group_1_binding_0_fs.offsets[_e20];
            float span = max((upper - lower), 1e-6);
            float local = clamp(((t - lower) / span), 0.0, 1.0);
            if ((t >= lower)) {
                int _e34 = i;
                vec4 _e38 = _group_1_binding_0_fs.stops[(_e34 - 1)];
                int _e41 = i;
                vec4 _e43 = _group_1_binding_0_fs.stops[_e41];
                result = mix(_e38, _e43, local);
            }
        }
    }
    vec4 _e48 = result;
    return _e48;
}

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

vec4 gradient_color(float t_2, int count_1) {
    if ((count_1 <= 0)) {
        vec4 _e9 = textureLod(_group_0_binding_0_fs, vec2(vec2(t_2, 0.5)), 0.0);
        return _e9;
    }
    vec4 _e10 = sample_stops(t_2, count_1);
    return _e10;
}

vec2 to_gradient_space(vec2 clip) {
    vec4 _e3 = _group_1_binding_0_fs.geometry;
    vec2 delta = (clip - _e3.xy);
    float _e9 = _group_1_binding_0_fs.to_local.x;
    float _e13 = _group_1_binding_0_fs.to_local.y;
    vec2 column0_ = vec2(_e9, _e13);
    float _e18 = _group_1_binding_0_fs.to_local.z;
    float _e22 = _group_1_binding_0_fs.to_local.w;
    vec2 column1_ = vec2(_e18, _e22);
    return ((column0_ * delta.x) + (column1_ * delta.y));
}

vec2 snapped(vec2 coord) {
    float _e4 = _group_1_binding_0_fs.params.z;
    float _e10 = _group_1_binding_0_fs.params.z;
    if (((_e4 < 0.5) || (_e10 > 1.5))) {
        return coord;
    }
    vec2 size = vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy));
    return ((floor((coord * size)) + vec2(0.5)) / size);
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

vec4 cubic(vec2 coord_1, vec2 low, vec2 high) {
    vec4 total = vec4(0.0);
    int j = -1;
    int i_1 = 0;
    vec2 size_1 = vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy));
    vec2 position_1 = ((coord_1 * size_1) - vec2(0.5));
    vec2 base = floor(position_1);
    vec2 offset = (position_1 - base);
    bool loop_init_1 = true;
    while(true) {
        if (!loop_init_1) {
            int _e57 = j;
            j = (_e57 + 1);
        }
        loop_init_1 = false;
        int _e17 = j;
        if ((_e17 <= 2)) {
        } else {
            break;
        }
        {
            int _e20 = j;
            float _e24 = cubic_weight((float(_e20) - offset.y));
            i_1 = -1;
            bool loop_init_2 = true;
            while(true) {
                if (!loop_init_2) {
                    int _e54 = i_1;
                    i_1 = (_e54 + 1);
                }
                loop_init_2 = false;
                int _e27 = i_1;
                if ((_e27 <= 2)) {
                } else {
                    break;
                }
                {
                    int _e30 = i_1;
                    float _e34 = cubic_weight((float(_e30) - offset.x));
                    float weight = (_e34 * _e24);
                    int _e36 = i_1;
                    int _e38 = j;
                    vec2 texel_2 = clamp((((base + vec2(float(_e36), float(_e38))) + vec2(0.5)) / size_1), low, high);
                    vec4 _e47 = total;
                    vec4 _e51 = textureLod(_group_0_binding_0_fs, vec2(texel_2), 0.0);
                    total = (_e47 + (_e51 * weight));
                }
            }
        }
    }
    float _e61 = total.w;
    float alpha = clamp(_e61, 0.0, 1.0);
    vec4 _e65 = total;
    return vec4(clamp(_e65.xyz, vec3(0.0), vec3(alpha)), alpha);
}

vec4 sampled(vec2 coord_2, vec2 low_1, vec2 high_1) {
    float _e6 = _group_1_binding_0_fs.params.z;
    if ((_e6 > 1.5)) {
        vec4 _e9 = cubic(coord_2, low_1, high_1);
        return _e9;
    }
    vec2 _e12 = snapped(coord_2);
    vec4 _e14 = textureLod(_group_0_binding_0_fs, vec2(_e12), 0.0);
    return _e14;
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

vec4 sample_mesh(vec2 uv_2) {
    vec4 texel = vec4(0.0);
    float tile_2 = _group_1_binding_0_fs.geometry.y;
    vec2 _e5 = tile_uv(uv_2, tile_2);
    vec4 _e10 = sampled(_e5, vec2(0.0), vec2(1.0));
    texel = _e10;
    if (((tile_2 > 1.5) && (tile_2 < 2.5))) {
        if ((any(lessThan(uv_2, vec2(0.0))) || any(greaterThan(uv_2, vec2(1.0))))) {
            texel = vec4(0.0);
        }
    }
    vec4 tint_1 = _group_1_binding_0_fs.stops[0];
    vec4 premultiplied_1 = vec4((tint_1.xyz * tint_1.w), tint_1.w);
    vec4 _e37 = texel;
    float _e42 = _group_1_binding_0_fs.geometry.x;
    return ((_e37 * premultiplied_1) * _e42);
}

vec4 sample_image(vec2 clip_1) {
    vec2 coord_3 = vec2(0.0);
    vec4 texel_1 = vec4(0.0);
    vec2 _e1 = to_gradient_space(clip_1);
    float tile_3 = _group_1_binding_0_fs.geometry.w;
    vec2 _e6 = tile_uv(_e1, tile_3);
    coord_3 = _e6;
    vec4 source = _group_1_binding_0_fs.stops[0];
    vec2 _e13 = coord_3;
    coord_3 = (source.xy + (_e13 * (source.zw - source.xy)));
    vec2 half_texel = (vec2(0.5) / vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy)));
    vec2 low_2 = min((source.xy + half_texel), (source.zw - half_texel));
    vec2 high_2 = max((source.xy + half_texel), (source.zw - half_texel));
    vec2 _e35 = coord_3;
    coord_3 = clamp(_e35, low_2, high_2);
    vec2 _e37 = coord_3;
    vec4 _e38 = sampled(_e37, low_2, high_2);
    texel_1 = _e38;
    if (((tile_3 > 1.5) && (tile_3 < 2.5))) {
        bool outside = (any(lessThan(_e1, vec2(0.0))) || any(greaterThan(_e1, vec2(1.0))));
        if (outside) {
            texel_1 = vec4(0.0);
        }
    }
    vec4 tint_2 = _group_1_binding_0_fs.stops[1];
    vec4 premultiplied_2 = vec4((tint_2.xyz * tint_2.w), tint_2.w);
    vec4 _e65 = texel_1;
    float _e70 = _group_1_binding_0_fs.geometry.z;
    return ((_e65 * premultiplied_2) * _e70);
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

float outline_if_asked(float distance_1) {
    float width_1 = _group_1_binding_0_fs.params.w;
    if ((width_1 <= 0.0)) {
        return distance_1;
    }
    return (abs(distance_1) - (width_1 * 0.5));
}

float rounded_rect_distance(vec2 point, vec2 half_size, float radius) {
    vec2 q = ((abs(point) - half_size) + vec2(radius));
    return ((min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0)))) - radius);
}

vec4 rounded_rect_coverage(vec2 clip_2) {
    vec2 _e1 = to_gradient_space(clip_2);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 half_size_1 = _e4.zw;
    float _e9 = _group_1_binding_0_fs.params.z;
    float radius_1 = clamp(_e9, 0.0, min(half_size_1.x, half_size_1.y));
    float _e15 = rounded_rect_distance(_e1, half_size_1, radius_1);
    float _e16 = dFdx(_e15);
    float _e17 = dFdy(_e15);
    vec2 gradient = vec2(_e16, _e17);
    float width_2 = length(gradient);
    float _e25 = _group_1_binding_0_fs.params.w;
    float _e26 = coverage_of(_e15, max(width_2, 1e-6), _e25);
    vec4 tint_3 = _group_1_binding_0_fs.stops[0];
    float alpha_1 = (tint_3.w * _e26);
    return vec4((tint_3.xyz * alpha_1), alpha_1);
}

vec4 ellipse_coverage(vec2 clip_3) {
    float stroke = 0.0;
    vec2 _e1 = to_gradient_space(clip_3);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 axes = max(_e4.zw, vec2(1e-6));
    float implicit = (length((_e1 / axes)) - 1.0);
    float _e13 = dFdx(implicit);
    float _e14 = dFdy(implicit);
    vec2 gradient_1 = vec2(_e13, _e14);
    float per_pixel_1 = max(length(gradient_1), 1e-6);
    float _e22 = _group_1_binding_0_fs.params.w;
    stroke = _e22;
    float _e24 = stroke;
    if ((_e24 > 0.0)) {
        float k1_ = max(length((_e1 / axes)), 1e-6);
        float k2_ = length((_e1 / (axes * axes)));
        float _e34 = stroke;
        stroke = ((_e34 * k2_) / k1_);
    }
    float _e37 = stroke;
    float _e38 = coverage_of(implicit, per_pixel_1, _e37);
    vec4 tint_4 = _group_1_binding_0_fs.stops[0];
    float alpha_2 = (tint_4.w * _e38);
    return vec4((tint_4.xyz * alpha_2), alpha_2);
}

vec4 blur_along_axis(vec2 clip_4) {
    vec4 total_1 = vec4(0.0);
    float weight_sum = 0.0;
    float i_2 = 0.0;
    vec2 _e1 = to_gradient_space(clip_4);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 step_ = _e4.zw;
    float _e9 = _group_1_binding_0_fs.params.z;
    float sigma = max(_e9, 0.0001);
    float reach = (sigma * 3.0);
    float spread = max((reach / 32.0), 1.0);
    float taps = min(ceil((reach / spread)), 32.0);
    float denominator = (-0.5 / (sigma * sigma));
    i_2 = -(taps);
    while(true) {
        float _e31 = i_2;
        if ((_e31 > taps)) {
            break;
        }
        float _e33 = i_2;
        float offset_1 = (_e33 * spread);
        float weight_1 = exp(((offset_1 * offset_1) * denominator));
        vec2 coord_4 = clamp((_e1 + (step_ * offset_1)), vec2(0.0), vec2(1.0));
        vec4 _e45 = total_1;
        vec4 _e49 = textureLod(_group_0_binding_0_fs, vec2(coord_4), 0.0);
        total_1 = (_e45 + (_e49 * weight_1));
        float _e52 = weight_sum;
        weight_sum = (_e52 + weight_1);
        float _e54 = i_2;
        i_2 = (_e54 + 1.0);
    }
    vec4 _e57 = total_1;
    float _e58 = weight_sum;
    return (_e57 / vec4(max(_e58, 1e-6)));
}

vec4 sample_or_nothing(vec2 uv_3) {
    if ((any(lessThan(uv_3, vec2(0.0))) || any(greaterThan(uv_3, vec2(1.0))))) {
        return vec4(0.0);
    }
    vec4 _e15 = textureLod(_group_0_binding_0_fs, vec2(uv_3), 0.0);
    return _e15;
}

vec4 morphology_along_axis(vec2 clip_5) {
    vec4 best = vec4(0.0);
    float i_3 = 1.0;
    vec2 _e1 = to_gradient_space(clip_5);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 step_1 = _e4.zw;
    float taps_1 = _group_1_binding_0_fs.params.z;
    float _e13 = _group_1_binding_0_fs.params.w;
    bool dilate = (_e13 > 0.5);
    vec4 _e16 = sample_or_nothing(_e1);
    best = _e16;
    while(true) {
        float _e20 = i_3;
        if ((_e20 > taps_1)) {
            break;
        }
        float _e22 = i_3;
        vec4 _e25 = sample_or_nothing((_e1 + (step_1 * _e22)));
        float _e26 = i_3;
        vec4 _e29 = sample_or_nothing((_e1 - (step_1 * _e26)));
        if (dilate) {
            vec4 _e30 = best;
            best = max(_e30, max(_e25, _e29));
        } else {
            vec4 _e33 = best;
            best = min(_e33, min(_e25, _e29));
        }
        float _e36 = i_3;
        i_3 = (_e36 + 1.0);
    }
    vec4 _e39 = best;
    return _e39;
}

vec3 linear_to_srgb(vec3 c) {
    vec3 low_3 = (c * 12.92);
    vec3 high_3 = ((1.055 * pow(max(c, vec3(0.0)), vec3(0.41666666))) - vec3(0.055));
    return mix(high_3, low_3, lessThanEqual(c, vec3(0.0031308)));
}

vec3 srgb_to_linear(vec3 c_1) {
    vec3 low_4 = (c_1 / vec3(12.92));
    vec3 high_4 = pow(((max(c_1, vec3(0.0)) + vec3(0.055)) / vec3(1.055)), vec3(2.4));
    return mix(high_4, low_4, lessThanEqual(c_1, vec3(0.04045)));
}

vec4 filtered(vec4 premultiplied) {
    vec4 color = vec4(0.0);
    vec4 out_1 = vec4(0.0);
    float kind = _group_1_binding_0_fs.filter_params.x;
    if ((kind < 0.5)) {
        return premultiplied;
    }
    bool straight = (kind > 1.5);
    color = premultiplied;
    if (straight) {
        float _e11 = color.w;
        float alpha_3 = max(_e11, 1e-6);
        vec4 _e14 = color;
        float _e19 = color.w;
        color = vec4((_e14.xyz / vec3(alpha_3)), _e19);
    }
    if ((kind > 2.5)) {
        if ((kind < 3.5)) {
            vec4 _e26 = color;
            vec3 _e28 = linear_to_srgb(_e26.xyz);
            float _e30 = color.w;
            out_1 = vec4(_e28, _e30);
        } else {
            vec4 _e32 = color;
            vec3 _e34 = srgb_to_linear(_e32.xyz);
            float _e36 = color.w;
            out_1 = vec4(_e34, _e36);
        }
    } else {
        vec4 _e41 = _group_1_binding_0_fs.recolor[0];
        float _e43 = color.x;
        vec4 _e48 = _group_1_binding_0_fs.recolor[1];
        float _e50 = color.y;
        vec4 _e56 = _group_1_binding_0_fs.recolor[2];
        float _e58 = color.z;
        vec4 _e64 = _group_1_binding_0_fs.recolor[3];
        float _e66 = color.w;
        vec4 _e71 = _group_1_binding_0_fs.filter_offset;
        out_1 = (((((_e41 * _e43) + (_e48 * _e50)) + (_e56 * _e58)) + (_e64 * _e66)) + _e71);
    }
    if (straight) {
        vec4 _e73 = out_1;
        out_1 = clamp(_e73, vec4(0.0), vec4(1.0));
        vec4 _e79 = out_1;
        float _e82 = out_1.w;
        float _e85 = out_1.w;
        return vec4((_e79.xyz * _e82), _e85);
    }
    float _e88 = out_1.w;
    float alpha_4 = clamp(_e88, 0.0, 1.0);
    vec4 _e92 = out_1;
    return vec4(clamp(_e92.xyz, vec3(0.0), vec3(alpha_4)), alpha_4);
}

vec4 shade(VertexOutput in_1) {
    vec4 color_1 = vec4(0.0);
    float t_3 = 0.0;
    bool covered = false;
    vec4 _e4 = _group_1_binding_0_fs.stops[0];
    color_1 = _e4;
    float kind_1 = _group_1_binding_0_fs.params.y;
    float _e13 = _group_1_binding_0_fs.params.x;
    int count_2 = int(_e13);
    if (((kind_1 > 0.5) && (kind_1 < 1.5))) {
        vec4 _e22 = _group_1_binding_0_fs.geometry;
        vec2 axis = _e22.zw;
        float length_squared = max(dot(axis, axis), 1e-6);
        vec2 _e28 = to_gradient_space(in_1.clip);
        float t_5 = (dot(_e28, axis) / length_squared);
        float _e34 = _group_1_binding_0_fs.params.z;
        vec2 _e35 = tile_gradient(t_5, _e34);
        vec4 _e37 = gradient_color(_e35.x, count_2);
        color_1 = (_e37 * _e35.y);
    } else {
        if (((kind_1 > 1.5) && (kind_1 < 2.5))) {
            vec2 _e46 = to_gradient_space(in_1.clip);
            float _e51 = _group_1_binding_0_fs.params.z;
            vec2 _e52 = tile_gradient(length(_e46), _e51);
            vec4 _e54 = gradient_color(_e52.x, count_2);
            color_1 = (_e54 * _e52.y);
        } else {
            if (((kind_1 > 2.5) && (kind_1 < 3.5))) {
                vec2 _e63 = to_gradient_space(in_1.clip);
                float angle = atan(_e63.y, _e63.x);
                float start_angle = _group_1_binding_0_fs.geometry.z;
                float _e74 = _group_1_binding_0_fs.geometry.w;
                float sweep = max((_e74 - start_angle), 1e-6);
                float delta_1 = (angle - start_angle);
                float ahead = (delta_1 - (6.2831855 * floor((delta_1 / 6.2831855))));
                float _e88 = _group_1_binding_0_fs.params.z;
                vec2 _e89 = tile_gradient((ahead / sweep), _e88);
                vec4 _e91 = gradient_color(_e89.x, count_2);
                color_1 = (_e91 * _e89.y);
            } else {
                if (((kind_1 > 8.5) && (kind_1 < 9.5))) {
                    vec2 _e100 = to_gradient_space(in_1.clip);
                    float separation = _group_1_binding_0_fs.params.w;
                    float r0_ = _group_1_binding_0_fs.geometry.z;
                    float dr = _group_1_binding_0_fs.geometry.w;
                    float a = ((separation * separation) - (dr * dr));
                    float b = ((_e100.x * separation) + (r0_ * dr));
                    float c_2 = (dot(_e100, _e100) - (r0_ * r0_));
                    float magnitude = max((separation * separation), (dr * dr));
                    if ((abs(a) > (magnitude * 1e-5))) {
                        float disc = ((b * b) - (a * c_2));
                        if ((disc >= 0.0)) {
                            float root = sqrt(disc);
                            float far = max(((b + root) / a), ((b - root) / a));
                            float near = min(((b + root) / a), ((b - root) / a));
                            if (((r0_ + (far * dr)) >= 0.0)) {
                                t_3 = far;
                                covered = true;
                            } else {
                                if (((r0_ + (near * dr)) >= 0.0)) {
                                    t_3 = near;
                                    covered = true;
                                }
                            }
                        }
                    } else {
                        if ((abs(b) > 1e-6)) {
                            float only = (c_2 / (2.0 * b));
                            if (((r0_ + (only * dr)) >= 0.0)) {
                                t_3 = only;
                                covered = true;
                            }
                        }
                    }
                    float _e171 = t_3;
                    float _e175 = _group_1_binding_0_fs.params.z;
                    vec2 _e176 = tile_gradient(_e171, _e175);
                    vec4 _e178 = gradient_color(_e176.x, count_2);
                    bool _e183 = covered;
                    color_1 = ((_e178 * _e176.y) * (_e183 ? 1.0 : 0.0));
                }
            }
        }
    }
    if (((kind_1 > 3.5) && (kind_1 < 4.5))) {
        vec4 _e192 = sample_image(in_1.clip);
        return _e192;
    }
    if (((kind_1 > 7.5) && (kind_1 < 8.5))) {
        vec4 _e199 = ellipse_coverage(in_1.clip);
        return _e199;
    }
    if (((kind_1 > 6.5) && (kind_1 < 7.5))) {
        vec4 _e206 = rounded_rect_coverage(in_1.clip);
        return _e206;
    }
    if (((kind_1 > 5.5) && (kind_1 < 6.5))) {
        vec4 _e213 = blur_along_axis(in_1.clip);
        return _e213;
    }
    if (((kind_1 > 10.5) && (kind_1 < 11.5))) {
        vec4 _e220 = morphology_along_axis(in_1.clip);
        return _e220;
    }
    if (((kind_1 > 9.5) && (kind_1 < 10.5))) {
        vec4 _e227 = sample_mesh(in_1.uv);
        return _e227;
    }
    if (((kind_1 > 4.5) && (kind_1 < 5.5))) {
        vec4 _e237 = textureLod(_group_0_binding_0_fs, vec2(in_1.uv), 0.0);
        float coverage = _e237.x;
        vec4 tint_5 = _group_1_binding_0_fs.stops[0];
        float alpha_5 = (tint_5.w * coverage);
        return vec4((tint_5.xyz * alpha_5), alpha_5);
    }
    vec4 _e248 = color_1;
    float _e251 = color_1.w;
    float _e254 = color_1.w;
    return vec4((_e248.xyz * _e251), _e254);
}

void main() {
    VertexOutput in_ = VertexOutput(gl_FragCoord, _vs2fs_location0, _vs2fs_location1, _vs2fs_location2);
    vec4 _e1 = shade(in_);
    vec4 _e4 = filtered((_e1 * in_.tint));
    _fs2p_location0 = _e4;
    return;
}

