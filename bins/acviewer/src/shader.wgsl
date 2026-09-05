struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
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
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = model.m * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    out.normal = (model.m * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    if (tex.a < 0.5) {
        discard;
    }
    let n = normalize(in.normal);
    let diffuse = max(dot(n, normalize(globals.light_dir.xyz)), 0.0);
    let light = globals.ambient.rgb + diffuse * (1.0 - globals.ambient.rgb);
    return vec4<f32>(tex.rgb * in.color.rgb * light, tex.a * in.color.a);
}

// ---- Outdoor terrain -------------------------------------------------------
// A cell is its base texture with up to three terrain overlays and two road
// overlays painted over it, each through an alpha map (black where the
// overlay shows) rotated by quarter turns to fit the corners it covers.
// Overlay word: texture layer | alpha layer << 8 | rotation << 16 | 1 << 31.
@group(1) @binding(2) var t_layers: texture_2d_array<f32>;
@group(1) @binding(3) var t_alphas: texture_2d_array<f32>;
@group(1) @binding(4) var s_layers: sampler;
@group(1) @binding(5) var s_alphas: sampler;

struct TerrainIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) cell_uv: vec2<f32>,
    @location(5) layers: vec4<u32>,
    @location(6) roads: vec2<u32>,
};
struct TerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) cell_uv: vec2<f32>,
    @location(4) @interpolate(flat) layers: vec4<u32>,
    @location(5) @interpolate(flat) roads: vec2<u32>,
};

@vertex
fn vs_terrain(in: TerrainIn) -> TerrainOut {
    var out: TerrainOut;
    let world = model.m * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    out.normal = (model.m * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.color = in.color;
    out.cell_uv = in.cell_uv;
    out.layers = in.layers;
    out.roads = in.roads;
    return out;
}

// Quarter turns anticlockwise (seen from above) of a cell's alpha map:
// texel (0,0) is the cell's north-west corner, and each turn moves it to
// the next corner SW -> SE -> NE -> NW.
fn rotate_uv(uv: vec2<f32>, r: u32) -> vec2<f32> {
    switch r {
        case 1u: { return vec2<f32>(1.0 - uv.y, uv.x); }
        case 2u: { return vec2<f32>(1.0 - uv.x, 1.0 - uv.y); }
        case 3u: { return vec2<f32>(uv.y, 1.0 - uv.x); }
        default: { return uv; }
    }
}

// Alpha-map value of an overlay at this point: 1 keeps what is beneath.
fn overlay_mask(word: u32, cell_uv: vec2<f32>) -> f32 {
    let alpha = (word >> 8u) & 0xFFu;
    let rot = (word >> 16u) & 3u;
    let present = (word >> 31u) != 0u;
    let a = textureSample(t_alphas, s_alphas, rotate_uv(cell_uv, rot), alpha).r;
    return select(1.0, a, present);
}

@fragment
fn fs_terrain(in: TerrainOut) -> @location(0) vec4<f32> {
    var color = textureSample(t_layers, s_layers, in.uv, in.layers.x & 0xFFu).rgb;
    // Terrain overlays, in order, each over the result so far.
    for (var i = 1u; i < 4u; i++) {
        let word = in.layers[i];
        let tex = textureSample(t_layers, s_layers, in.uv, word & 0xFFu).rgb;
        color = mix(tex, color, overlay_mask(word, in.cell_uv));
    }
    // Roads last; two road masks cover the union of their shapes.
    let road = textureSample(t_layers, s_layers, in.uv, in.roads.x & 0xFFu).rgb;
    let a = overlay_mask(in.roads.x, in.cell_uv) * overlay_mask(in.roads.y, in.cell_uv);
    color = mix(road, color, a);
    let n = normalize(in.normal);
    let diffuse = max(dot(n, normalize(globals.light_dir.xyz)), 0.0);
    let light = globals.ambient.rgb + diffuse * (1.0 - globals.ambient.rgb);
    return vec4<f32>(color * in.color.rgb * light, 1.0);
}
