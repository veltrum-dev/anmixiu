struct DrawInstance {
    @location(0) color: vec4<f32>,
    @location(1) bounds: vec4<f32>,
    @location(2) clip_rect: vec4<f32>,
    @location(3) misc: vec4<f32>,
    @location(4) uv_rect: vec4<f32>,
    @location(5) draw_flags: vec4<f32>,
    @location(6) viewport: vec2<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) world: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) clip_rect: vec4<f32>,
    @location(4) misc: vec4<f32>,
    @location(5) uv: vec2<f32>,
    @location(6) draw_flags: vec4<f32>,
    @location(7) bounds_size: vec2<f32>,
};

@group(0) @binding(0)
var atlas_texture: texture_2d<f32>;

@group(0) @binding(1)
var atlas_sampler: sampler;

@vertex
fn vertex_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: DrawInstance,
) -> VertexOut {
    let unit_vertices = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let unit = unit_vertices[vertex_index];
    let world = instance.bounds.xy + unit * instance.bounds.zw;

    var out: VertexOut;
    out.position = vec4<f32>(
        world.x / instance.viewport.x * 2.0 - 1.0,
        1.0 - world.y / instance.viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.local = unit * instance.bounds.zw;
    out.world = world;
    out.color = instance.color;
    out.clip_rect = instance.clip_rect;
    out.misc = instance.misc;
    out.uv = instance.uv_rect.xy + unit * instance.uv_rect.zw;
    out.draw_flags = instance.draw_flags;
    out.bounds_size = instance.bounds.zw;
    return out;
}

fn rounded_distance(point: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(point - size * 0.5) - (size * 0.5 - radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn outside_clip(in: VertexOut) -> bool {
    if in.misc.w <= 0.5 {
        return false;
    }
    let clip_local = in.world - in.clip_rect.xy;
    if any(clip_local < vec2<f32>(0.0)) || any(clip_local >= in.clip_rect.zw) {
        return true;
    }
    return in.misc.y > 0.0 && rounded_distance(clip_local, in.clip_rect.zw, in.misc.y) > 0.0;
}

fn srgb_channel_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        return channel / 12.92;
    }
    return pow((channel + 0.055) / 1.055, 2.4);
}

@fragment
fn fragment_main(in: VertexOut) -> @location(0) vec4<f32> {
    let distance = rounded_distance(in.local, in.bounds_size, in.misc.x);
    if in.draw_flags.y > 0.5 {
        if (in.misc.x > 0.0 && distance > 0.0) || distance < -in.draw_flags.x {
            discard;
        }
    } else if in.misc.x > 0.0 && distance > 0.0 {
        discard;
    }
    if outside_clip(in) {
        discard;
    }
    var color = in.color;
    if in.misc.z > 0.5 {
        color.a *= textureSample(atlas_texture, atlas_sampler, in.uv).r;
        if color.a <= 0.0 {
            discard;
        }
    }
    return vec4<f32>(
        srgb_channel_to_linear(color.r),
        srgb_channel_to_linear(color.g),
        srgb_channel_to_linear(color.b),
        color.a,
    );
}

@fragment
fn image_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    if in.misc.x > 0.0 && rounded_distance(in.local, in.bounds_size, in.misc.x) > 0.0 {
        discard;
    }
    if outside_clip(in) {
        discard;
    }
    return textureSample(atlas_texture, atlas_sampler, in.uv);
}

fn blur_sample(uv: vec2<f32>, transparent_edges: bool) -> vec4<f32> {
    if transparent_edges && (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    return textureSample(atlas_texture, atlas_sampler, uv);
}

@fragment
fn blur_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    let sigma = max(in.misc.x, 0.001);
    let radius = min(i32(ceil(sigma * 3.0)), 64);
    let transparent_edges = in.misc.z > 0.5;
    var accumulated = blur_sample(in.uv, transparent_edges);
    var total_weight = 1.0;
    for (var index = 1; index <= 64; index += 1) {
        if index > radius {
            break;
        }
        let offset = f32(index);
        let weight = exp(-(offset * offset) / (2.0 * sigma * sigma));
        let sample_offset = offset * in.draw_flags.zw;
        accumulated += blur_sample(in.uv + sample_offset, transparent_edges) * weight;
        accumulated += blur_sample(in.uv - sample_offset, transparent_edges) * weight;
        total_weight += weight * 2.0;
    }
    return accumulated / total_weight;
}
