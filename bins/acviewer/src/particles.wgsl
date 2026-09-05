// Particles: camera-facing quads, one instance each, textured through the
// same material bind group as the scene. Kept apart from shader.wgsl so
// the scene shader can change without touching this.

struct ParticleGlobals {
    view_proj: mat4x4<f32>,
    // World-space directions of the screen's x and y axes.
    right: vec4<f32>,
    up: vec4<f32>,
    camera: vec4<f32>,
    fog_color: vec4<f32>,
    // x: fog start, y: fog end (world units).
    fog_params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> pg: ParticleGlobals;
@group(1) @binding(0) var t_sprite: texture_2d<f32>;
@group(1) @binding(1) var s_sprite: sampler;

struct ParticleIn {
    @builtin(vertex_index) corner: u32,
    @location(0) center: vec3<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct ParticleOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_particle(in: ParticleIn) -> ParticleOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>(0.5, -0.5),
        vec2<f32>(0.5, 0.5),
        vec2<f32>(-0.5, -0.5),
        vec2<f32>(0.5, 0.5),
        vec2<f32>(-0.5, 0.5),
    );
    let c = corners[in.corner];
    let world = in.center + pg.right.xyz * (c.x * in.size.x) + pg.up.xyz * (c.y * in.size.y);
    var out: ParticleOut;
    out.clip = pg.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(c.x + 0.5, 0.5 - c.y);
    out.color = in.color;
    out.world = world;
    return out;
}

// 0 near the camera, 1 where fog is solid.
fn fog_amount(world: vec3<f32>) -> f32 {
    let d = distance(world, pg.camera.xyz);
    return clamp((d - pg.fog_params.x) / (pg.fog_params.y - pg.fog_params.x), 0.0, 1.0);
}

// Alpha-blended sprites (smoke, dust): the texture's alpha times the
// particle's opacity, fogged like any surface.
@fragment
fn fs_particle_alpha(in: ParticleOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_sprite, s_sprite, in.uv);
    let a = tex.a * in.color.a;
    if (a < 0.004) {
        discard;
    }
    let rgb = mix(tex.rgb * in.color.rgb, pg.fog_color.rgb, fog_amount(in.world));
    return vec4<f32>(rgb, a);
}

// Additive sprites (fire, glows): light added over what is behind, so
// the output is premultiplied and simply fades with distance.
@fragment
fn fs_particle_add(in: ParticleOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_sprite, s_sprite, in.uv);
    let a = tex.a * in.color.a * (1.0 - fog_amount(in.world));
    return vec4<f32>(tex.rgb * in.color.rgb * a, 0.0);
}
