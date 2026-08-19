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
layout(std140) uniform Paint_block_0Fragment { Paint _group_1_binding_0_fs; };

uniform highp sampler2D _group_0_binding_0_fs;

smooth in vec2 _vs2fs_location0;
smooth in vec2 _vs2fs_location1;
layout(location = 0) out vec4 _fs2p_location0;

vec4 sample_stops(float t_1, int count) {
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
            float local = clamp(((t_1 - lower) / span), 0.0, 1.0);
            if ((t_1 >= lower)) {
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

vec2 tile_gradient(float t_2, float tile) {
    if (((tile > 0.5) && (tile < 1.5))) {
        return vec2((t_2 - floor(t_2)), 1.0);
    }
    if (((tile > 1.5) && (tile < 2.5))) {
        float inside = (((t_2 >= 0.0) && (t_2 <= 1.0)) ? 1.0 : 0.0);
        return vec2(clamp(t_2, 0.0, 1.0), inside);
    }
    if ((tile > 2.5)) {
        return vec2((1.0 - abs((1.0 - (t_2 - (2.0 * floor((t_2 * 0.5))))))), 1.0);
    }
    return vec2(clamp(t_2, 0.0, 1.0), 1.0);
}

vec4 gradient_color(float t_3, int count_1) {
    if ((count_1 <= 0)) {
        vec4 _e9 = textureLod(_group_0_binding_0_fs, vec2(vec2(t_3, 0.5)), 0.0);
        return _e9;
    }
    vec4 _e10 = sample_stops(t_3, count_1);
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
    vec2 _e7 = tile_uv(uv_2, tile_2);
    vec4 _e9 = textureLod(_group_0_binding_0_fs, vec2(_e7), 0.0);
    texel = _e9;
    if (((tile_2 > 1.5) && (tile_2 < 2.5))) {
        if ((any(lessThan(uv_2, vec2(0.0))) || any(greaterThan(uv_2, vec2(1.0))))) {
            texel = vec4(0.0);
        }
    }
    vec4 tint = _group_1_binding_0_fs.stops[0];
    vec4 premultiplied = vec4((tint.xyz * tint.w), tint.w);
    vec4 _e36 = texel;
    float _e41 = _group_1_binding_0_fs.geometry.x;
    return ((_e36 * premultiplied) * _e41);
}

vec4 sample_image(vec2 clip_1) {
    vec2 coord = vec2(0.0);
    vec4 texel_1 = vec4(0.0);
    vec2 _e1 = to_gradient_space(clip_1);
    float tile_3 = _group_1_binding_0_fs.geometry.w;
    vec2 _e6 = tile_uv(_e1, tile_3);
    coord = _e6;
    vec4 source = _group_1_binding_0_fs.stops[0];
    vec2 _e13 = coord;
    coord = (source.xy + (_e13 * (source.zw - source.xy)));
    vec2 half_texel = (vec2(0.5) / vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy)));
    vec2 low = min((source.xy + half_texel), (source.zw - half_texel));
    vec2 high = max((source.xy + half_texel), (source.zw - half_texel));
    vec2 _e35 = coord;
    coord = clamp(_e35, low, high);
    vec2 _e39 = coord;
    vec4 _e41 = textureLod(_group_0_binding_0_fs, vec2(_e39), 0.0);
    texel_1 = _e41;
    if (((tile_3 > 1.5) && (tile_3 < 2.5))) {
        bool outside = (any(lessThan(_e1, vec2(0.0))) || any(greaterThan(_e1, vec2(1.0))));
        if (outside) {
            texel_1 = vec4(0.0);
        }
    }
    vec4 tint_1 = _group_1_binding_0_fs.stops[1];
    vec4 premultiplied_1 = vec4((tint_1.xyz * tint_1.w), tint_1.w);
    vec4 _e68 = texel_1;
    float _e73 = _group_1_binding_0_fs.geometry.z;
    return ((_e68 * premultiplied_1) * _e73);
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
    vec4 tint_2 = _group_1_binding_0_fs.stops[0];
    float alpha = (tint_2.w * _e26);
    return vec4((tint_2.xyz * alpha), alpha);
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
    vec4 tint_3 = _group_1_binding_0_fs.stops[0];
    float alpha_1 = (tint_3.w * _e38);
    return vec4((tint_3.xyz * alpha_1), alpha_1);
}

vec4 blur_along_axis(vec2 clip_4) {
    vec4 total = vec4(0.0);
    float weight_sum = 0.0;
    float i_1 = 0.0;
    vec2 _e1 = to_gradient_space(clip_4);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 step_ = _e4.zw;
    float _e9 = _group_1_binding_0_fs.params.z;
    float sigma = max(_e9, 0.0001);
    float reach = (sigma * 3.0);
    float spread = max((reach / 32.0), 1.0);
    float taps = min(ceil((reach / spread)), 32.0);
    float denominator = (-0.5 / (sigma * sigma));
    i_1 = -(taps);
    while(true) {
        float _e31 = i_1;
        if ((_e31 > taps)) {
            break;
        }
        float _e33 = i_1;
        float offset = (_e33 * spread);
        float weight = exp(((offset * offset) * denominator));
        vec2 coord_1 = clamp((_e1 + (step_ * offset)), vec2(0.0), vec2(1.0));
        vec4 _e45 = total;
        vec4 _e49 = textureLod(_group_0_binding_0_fs, vec2(coord_1), 0.0);
        total = (_e45 + (_e49 * weight));
        float _e52 = weight_sum;
        weight_sum = (_e52 + weight);
        float _e54 = i_1;
        i_1 = (_e54 + 1.0);
    }
    vec4 _e57 = total;
    float _e58 = weight_sum;
    return (_e57 / vec4(max(_e58, 1e-6)));
}

void main() {
    VertexOutput in_ = VertexOutput(gl_FragCoord, _vs2fs_location0, _vs2fs_location1);
    vec4 color = vec4(0.0);
    float t = 0.0;
    bool covered = false;
    vec4 _e4 = _group_1_binding_0_fs.stops[0];
    color = _e4;
    float kind = _group_1_binding_0_fs.params.y;
    float _e13 = _group_1_binding_0_fs.params.x;
    int count_2 = int(_e13);
    if (((kind > 0.5) && (kind < 1.5))) {
        vec4 _e22 = _group_1_binding_0_fs.geometry;
        vec2 axis = _e22.zw;
        float length_squared = max(dot(axis, axis), 1e-6);
        vec2 _e28 = to_gradient_space(in_.clip);
        float t_4 = (dot(_e28, axis) / length_squared);
        float _e34 = _group_1_binding_0_fs.params.z;
        vec2 _e35 = tile_gradient(t_4, _e34);
        vec4 _e37 = gradient_color(_e35.x, count_2);
        color = (_e37 * _e35.y);
    } else {
        if (((kind > 1.5) && (kind < 2.5))) {
            vec2 _e46 = to_gradient_space(in_.clip);
            float _e51 = _group_1_binding_0_fs.params.z;
            vec2 _e52 = tile_gradient(length(_e46), _e51);
            vec4 _e54 = gradient_color(_e52.x, count_2);
            color = (_e54 * _e52.y);
        } else {
            if (((kind > 2.5) && (kind < 3.5))) {
                vec2 _e63 = to_gradient_space(in_.clip);
                float angle = atan(_e63.y, _e63.x);
                float start_angle = _group_1_binding_0_fs.geometry.z;
                float _e74 = _group_1_binding_0_fs.geometry.w;
                float sweep = max((_e74 - start_angle), 1e-6);
                float delta_1 = (angle - start_angle);
                float ahead = (delta_1 - (6.2831855 * floor((delta_1 / 6.2831855))));
                float _e88 = _group_1_binding_0_fs.params.z;
                vec2 _e89 = tile_gradient((ahead / sweep), _e88);
                vec4 _e91 = gradient_color(_e89.x, count_2);
                color = (_e91 * _e89.y);
            } else {
                if (((kind > 8.5) && (kind < 9.5))) {
                    vec2 _e100 = to_gradient_space(in_.clip);
                    float separation = _group_1_binding_0_fs.params.w;
                    float r0_ = _group_1_binding_0_fs.geometry.z;
                    float dr = _group_1_binding_0_fs.geometry.w;
                    float a = ((separation * separation) - (dr * dr));
                    float b = ((_e100.x * separation) + (r0_ * dr));
                    float c = (dot(_e100, _e100) - (r0_ * r0_));
                    float magnitude = max((separation * separation), (dr * dr));
                    if ((abs(a) > (magnitude * 1e-5))) {
                        float disc = ((b * b) - (a * c));
                        if ((disc >= 0.0)) {
                            float root = sqrt(disc);
                            float far = max(((b + root) / a), ((b - root) / a));
                            float near = min(((b + root) / a), ((b - root) / a));
                            if (((r0_ + (far * dr)) >= 0.0)) {
                                t = far;
                                covered = true;
                            } else {
                                if (((r0_ + (near * dr)) >= 0.0)) {
                                    t = near;
                                    covered = true;
                                }
                            }
                        }
                    } else {
                        if ((abs(b) > 1e-6)) {
                            float only = (c / (2.0 * b));
                            if (((r0_ + (only * dr)) >= 0.0)) {
                                t = only;
                                covered = true;
                            }
                        }
                    }
                    float _e171 = t;
                    float _e175 = _group_1_binding_0_fs.params.z;
                    vec2 _e176 = tile_gradient(_e171, _e175);
                    vec4 _e178 = gradient_color(_e176.x, count_2);
                    bool _e183 = covered;
                    color = ((_e178 * _e176.y) * (_e183 ? 1.0 : 0.0));
                }
            }
        }
    }
    if (((kind > 3.5) && (kind < 4.5))) {
        vec4 _e192 = sample_image(in_.clip);
        _fs2p_location0 = _e192;
        return;
    }
    if (((kind > 7.5) && (kind < 8.5))) {
        vec4 _e199 = ellipse_coverage(in_.clip);
        _fs2p_location0 = _e199;
        return;
    }
    if (((kind > 6.5) && (kind < 7.5))) {
        vec4 _e206 = rounded_rect_coverage(in_.clip);
        _fs2p_location0 = _e206;
        return;
    }
    if (((kind > 5.5) && (kind < 6.5))) {
        vec4 _e213 = blur_along_axis(in_.clip);
        _fs2p_location0 = _e213;
        return;
    }
    if (((kind > 9.5) && (kind < 10.5))) {
        vec4 _e220 = sample_mesh(in_.uv);
        _fs2p_location0 = _e220;
        return;
    }
    if (((kind > 4.5) && (kind < 5.5))) {
        vec4 _e230 = textureLod(_group_0_binding_0_fs, vec2(in_.uv), 0.0);
        float coverage = _e230.x;
        vec4 tint_4 = _group_1_binding_0_fs.stops[0];
        float alpha_2 = (tint_4.w * coverage);
        _fs2p_location0 = vec4((tint_4.xyz * alpha_2), alpha_2);
        return;
    }
    vec4 _e241 = color;
    float _e244 = color.w;
    float _e247 = color.w;
    _fs2p_location0 = vec4((_e241.xyz * _e244), _e247);
    return;
}

