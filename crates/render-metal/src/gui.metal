#include <metal_stdlib>
using namespace metal;

struct DrawUniforms {
    float4 color;
    float4 bounds;
    float4 clip_rect;
    float4 misc;
    float4 uv_rect;
};

struct VertexOut {
    float4 position [[position]];
    float2 local;
    float2 world;
    float2 uv;
};

vertex VertexOut gui_vertex(
    uint vertex_id [[vertex_id]],
    const device float2 *unit_vertices [[buffer(0)]],
    constant DrawUniforms &draw [[buffer(1)]],
    constant float2 &viewport [[buffer(2)]]) {
    float2 unit = unit_vertices[vertex_id];
    float2 world = draw.bounds.xy + unit * draw.bounds.zw;
    VertexOut out;
    out.position = float4(
        world.x / viewport.x * 2.0 - 1.0,
        1.0 - world.y / viewport.y * 2.0,
        0.0,
        1.0);
    out.local = unit * draw.bounds.zw;
    out.world = world;
    out.uv = draw.uv_rect.xy + unit * draw.uv_rect.zw;
    return out;
}

float rounded_distance(float2 point, float2 size, float radius) {
    float2 q = abs(point - size * 0.5) - (size * 0.5 - radius);
    return length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fragment float4 gui_fragment(
    VertexOut in [[stage_in]],
    constant DrawUniforms &draw [[buffer(1)]],
    texture2d<float> atlas [[texture(0)]]) {
    if (draw.misc.x > 0.0 && rounded_distance(in.local, draw.bounds.zw, draw.misc.x) > 0.0) {
        discard_fragment();
    }
    if (draw.misc.w > 0.5) {
        float2 clip_local = in.world - draw.clip_rect.xy;
        if (any(clip_local < float2(0.0)) || any(clip_local >= draw.clip_rect.zw)) {
            discard_fragment();
        }
        if (draw.misc.y > 0.0 && rounded_distance(clip_local, draw.clip_rect.zw, draw.misc.y) > 0.0) {
            discard_fragment();
        }
    }
    float4 color = draw.color;
    if (draw.misc.z > 0.5) {
        constexpr sampler glyph_sampler(coord::normalized, address::clamp_to_edge, filter::linear);
        color.a *= atlas.sample(glyph_sampler, in.uv).r;
        if (color.a <= 0.0) {
            discard_fragment();
        }
    }
    return color;
}

fragment float4 border_fragment(
    VertexOut in [[stage_in]],
    constant DrawUniforms &draw [[buffer(1)]]) {
    float distance = rounded_distance(in.local, draw.bounds.zw, draw.misc.x);
    if ((draw.misc.x > 0.0 && distance > 0.0) || distance < -draw.uv_rect.x) {
        discard_fragment();
    }
    if (draw.misc.w > 0.5) {
        float2 clip_local = in.world - draw.clip_rect.xy;
        if (any(clip_local < float2(0.0)) || any(clip_local >= draw.clip_rect.zw)) {
            discard_fragment();
        }
        if (draw.misc.y > 0.0 && rounded_distance(clip_local, draw.clip_rect.zw, draw.misc.y) > 0.0) {
            discard_fragment();
        }
    }
    return draw.color;
}

fragment float4 image_fragment(
    VertexOut in [[stage_in]],
    constant DrawUniforms &draw [[buffer(1)]],
    texture2d<float> image [[texture(0)]]) {
    if (draw.misc.x > 0.0 && rounded_distance(in.local, draw.bounds.zw, draw.misc.x) > 0.0) {
        discard_fragment();
    }
    if (draw.misc.w > 0.5) {
        float2 clip_local = in.world - draw.clip_rect.xy;
        if (any(clip_local < float2(0.0)) || any(clip_local >= draw.clip_rect.zw)) {
            discard_fragment();
        }
        if (draw.misc.y > 0.0 && rounded_distance(clip_local, draw.clip_rect.zw, draw.misc.y) > 0.0) {
            discard_fragment();
        }
    }
    constexpr sampler image_sampler(coord::normalized, address::clamp_to_edge, filter::linear);
    return image.sample(image_sampler, in.uv);
}

fragment float4 blur_fragment(
    VertexOut in [[stage_in]],
    constant DrawUniforms &draw [[buffer(1)]],
    texture2d<float> image [[texture(0)]]) {
    constexpr sampler image_sampler(coord::normalized, address::clamp_to_edge, filter::linear);
    float sigma = max(draw.misc.x, 0.001);
    int radius = min(int(ceil(sigma * 3.0)), 64);
    float center_weight = 1.0;
    float4 accumulated = image.sample(image_sampler, in.uv) * center_weight;
    float total_weight = center_weight;
    for (int sample_index = 1; sample_index <= radius; sample_index += 2) {
        float first_offset = float(sample_index);
        float second_offset = float(min(sample_index + 1, radius));
        float first_weight = exp(
            -(first_offset * first_offset) / (2.0 * sigma * sigma));
        float second_weight = sample_index < radius
            ? exp(-(second_offset * second_offset) / (2.0 * sigma * sigma))
            : 0.0;
        float combined_weight = first_weight + second_weight;
        float combined_offset =
            (first_offset * first_weight + second_offset * second_weight) / combined_weight;
        float2 sample_offset = combined_offset * draw.misc.yz;
        accumulated += image.sample(image_sampler, in.uv + sample_offset) * combined_weight;
        accumulated += image.sample(image_sampler, in.uv - sample_offset) * combined_weight;
        total_weight += combined_weight * 2.0;
    }
    return accumulated / total_weight;
}
