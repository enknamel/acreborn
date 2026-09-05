struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // xyz: camera position, w: seconds since start.
    camera: vec4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
    sun_color: vec4<f32>,
    fog_color: vec4<f32>,
    // x: fog start, y: fog end (world units).
    fog_params: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    // rgb: tint, a: base opacity.
    water_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;
struct Model {
    m: mat4x4<f32>,
};
@group(2) @binding(0) var<uniform> model: Model;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = model.m * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    out.normal = (model.m * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.color = in.color;
    out.world = world.xyz;
    return out;
}

/// Linear distance fog towards the horizon colour.
fn fog(rgb: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let d = distance(world, globals.camera.xyz);
    let f = clamp((d - globals.fog_params.x) / (globals.fog_params.y - globals.fog_params.x), 0.0, 1.0);
    return mix(rgb, globals.fog_color.rgb, f);
}

fn lighting(n: vec3<f32>) -> vec3<f32> {
    let diffuse = max(dot(n, normalize(globals.light_dir.xyz)), 0.0);
    return globals.ambient.rgb + diffuse * globals.sun_color.rgb;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    if (tex.a < 0.5) {
        discard;
    }
    let n = normalize(in.normal);
    let rgb = tex.rgb * in.color.rgb * lighting(n);
    return vec4<f32>(fog(rgb, in.world), tex.a * in.color.a);
}

// ---- Sky: a full-screen triangle shaded by view direction -------------

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: SkyOut;
    out.clip = vec4<f32>(pos[i], 1.0, 1.0);
    out.ndc = pos[i];
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let near = globals.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = globals.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - near.xyz / near.w);
    let up = dir.z;
    let horizon = globals.sky_horizon.rgb;
    var col: vec3<f32>;
    if (up >= 0.0) {
        // Haze hugs the horizon; the blue deepens within ~15 degrees.
        col = mix(horizon, globals.sky_zenith.rgb, 1.0 - exp(-up * 7.0));
    } else {
        // Below the horizon (beyond the terrain): fog darkening downwards.
        col = mix(horizon, horizon * 0.8, smoothstep(0.0, 0.35, -up));
    }
    let s = max(dot(dir, normalize(globals.light_dir.xyz)), 0.0);
    col += globals.sun_color.rgb * (pow(s, 400.0) * 1.2 + pow(s, 6.0) * 0.12);
    return vec4<f32>(col, 1.0);
}

// ---- Water: translucent, rippled, reflecting the sky -------------------

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fs_water(in: VsOut) -> @location(0) vec4<f32> {
    let t = globals.camera.w;
    let uv1 = in.uv + vec2<f32>(t * 0.011, t * 0.007);
    let uv2 = in.uv * 0.63 - vec2<f32>(t * 0.006, -t * 0.009);
    let e = 0.004;
    let c1 = textureSample(t_diffuse, s_diffuse, uv1).rgb;
    let c2 = textureSample(t_diffuse, s_diffuse, uv2).rgb;
    let dx = lum(textureSample(t_diffuse, s_diffuse, uv1 + vec2<f32>(e, 0.0)).rgb)
        - lum(textureSample(t_diffuse, s_diffuse, uv1 - vec2<f32>(e, 0.0)).rgb)
        + lum(textureSample(t_diffuse, s_diffuse, uv2 + vec2<f32>(e, 0.0)).rgb)
        - lum(textureSample(t_diffuse, s_diffuse, uv2 - vec2<f32>(e, 0.0)).rgb);
    let dy = lum(textureSample(t_diffuse, s_diffuse, uv1 + vec2<f32>(0.0, e)).rgb)
        - lum(textureSample(t_diffuse, s_diffuse, uv1 - vec2<f32>(0.0, e)).rgb)
        + lum(textureSample(t_diffuse, s_diffuse, uv2 + vec2<f32>(0.0, e)).rgb)
        - lum(textureSample(t_diffuse, s_diffuse, uv2 - vec2<f32>(0.0, e)).rgb);
    let n = normalize(vec3<f32>(-dx * 3.0, -dy * 3.0, 1.0));
    let v = normalize(globals.camera.xyz - in.world);
    let l = normalize(globals.light_dir.xyz);
    let ripple = (c1 + c2) * 0.5;
    let tint = globals.water_color.rgb;
    let base = mix(tint, ripple * 1.5, 0.4) * lighting(n);
    let fresnel = 0.04 + 0.96 * pow(1.0 - max(dot(v, n), 0.0), 4.0);
    let sky = mix(globals.sky_horizon.rgb, globals.sky_zenith.rgb, clamp(v.z, 0.0, 1.0));
    let spec = pow(max(dot(reflect(-l, n), v), 0.0), 96.0) * globals.sun_color.rgb;
    let rgb = mix(base, sky, fresnel * 0.7) + spec;
    let alpha = clamp(globals.water_color.a + fresnel * 0.35, 0.0, 0.96);
    return vec4<f32>(fog(rgb, in.world), alpha);
}
