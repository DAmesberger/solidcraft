// Shared shading library: noise (crates/gpu/src/shading/mod.rs). Pure functions, `sc_` names,
// WebGL2-safe. Original lattice noise: every value is in [0, 1], seeded, and continuous in its
// point. The 3D hash and value noise are the path tracer's aging masks' (`aging.wgsl`), the 2D
// ones the viewport's former `pat_hash` and `pat_noise` at seed 0.

// Noise kinds and dimensions for `sc_noise` and `sc_fbm` (the `pattern.noise` node's params).
const sc_noise_kind_value: u32 = 0u;
const sc_noise_kind_gradient: u32 = 1u;
const sc_noise_kind_cellular: u32 = 2u;

// A lattice cell's hash in [0, 1].
fn sc_hash2(cell: vec2<i32>, seed: u32) -> f32 {
    var h = (bitcast<u32>(cell.x) * 1597334677u) ^ (bitcast<u32>(cell.y) * 3812015801u) ^ (seed * 0x9e3779b9u);
    h = h * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h) / 4294967295.0;
}

// A lattice cell's hash in [0, 1), periodic over 4096 cells on each axis.
fn sc_hash3(cell: vec3<i32>, seed: u32) -> f32 {
    let c = bitcast<vec3<u32>>(cell) & vec3<u32>(4095u);
    var h = (c.x * 0x8da6b343u) ^ (c.y * 0xd8163841u) ^ (c.z * 0xcb1ab31fu) ^ (seed * 0x9e3779b9u);
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return f32(h >> 8u) / 16777216.0;
}

// Value noise: the cell hashes blended by a smoothstep; equal to the hash on lattice points.
fn sc_value_noise2(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let c = vec2<i32>(i);
    let a = sc_hash2(c, seed);
    let b = sc_hash2(c + vec2<i32>(1, 0), seed);
    let d = sc_hash2(c + vec2<i32>(0, 1), seed);
    let e = sc_hash2(c + vec2<i32>(1, 1), seed);
    return mix(mix(a, b, w.x), mix(d, e, w.x), w.y);
}

fn sc_value_noise3(p: vec3<f32>, seed: u32) -> f32 {
    let base = vec3<i32>(floor(p));
    let f = fract(p);
    let w = f * f * (vec3<f32>(3.0) - 2.0 * f);
    let low = mix(mix(sc_hash3(base, seed), sc_hash3(base + vec3<i32>(1, 0, 0), seed), w.x),
        mix(sc_hash3(base + vec3<i32>(0, 1, 0), seed), sc_hash3(base + vec3<i32>(1, 1, 0), seed), w.x), w.y);
    let high = mix(mix(sc_hash3(base + vec3<i32>(0, 0, 1), seed), sc_hash3(base + vec3<i32>(1, 0, 1), seed), w.x),
        mix(sc_hash3(base + vec3<i32>(0, 1, 1), seed), sc_hash3(base + vec3<i32>(1, 1, 1), seed), w.x), w.y);
    return mix(low, high, w.z);
}

// A cell's unit gradient: an angle in 2D, a uniform direction on the sphere in 3D.
fn sc_gradient2(cell: vec2<i32>, seed: u32) -> vec2<f32> {
    let angle = 6.283185307179586 * sc_hash2(cell, seed);
    return vec2<f32>(cos(angle), sin(angle));
}

fn sc_gradient3(cell: vec3<i32>, seed: u32) -> vec3<f32> {
    let z = 2.0 * sc_hash3(cell, seed) - 1.0;
    let angle = 6.283185307179586 * sc_hash3(cell, seed ^ 0x68e31da4u);
    let r = sqrt(max(0.0, 1.0 - z * z));
    return vec3<f32>(r * cos(angle), r * sin(angle), z);
}

// Gradient noise with a quintic fade, mapped from its bound (√(d/4) for unit gradients in d
// dimensions) to [0, 1]: 0.5 on every lattice point.
fn sc_gradient_noise2(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * f * (f * (f * 6.0 - vec2<f32>(15.0)) + vec2<f32>(10.0));
    let c = vec2<i32>(i);
    let a = dot(sc_gradient2(c, seed), f);
    let b = dot(sc_gradient2(c + vec2<i32>(1, 0), seed), f - vec2<f32>(1.0, 0.0));
    let d = dot(sc_gradient2(c + vec2<i32>(0, 1), seed), f - vec2<f32>(0.0, 1.0));
    let e = dot(sc_gradient2(c + vec2<i32>(1, 1), seed), f - vec2<f32>(1.0, 1.0));
    let n = mix(mix(a, b, w.x), mix(d, e, w.x), w.y);
    return clamp(0.5 + n * 0.7071067811865476, 0.0, 1.0);
}

fn sc_gradient_noise3(p: vec3<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * f * (f * (f * 6.0 - vec3<f32>(15.0)) + vec3<f32>(10.0));
    let c = vec3<i32>(i);
    var corners: array<f32, 8>;
    for (var k = 0u; k < 8u; k += 1u) {
        let o = vec3<u32>(k & 1u, (k >> 1u) & 1u, (k >> 2u) & 1u);
        corners[k] = dot(sc_gradient3(c + vec3<i32>(o), seed), f - vec3<f32>(o));
    }
    let low = mix(mix(corners[0], corners[1], w.x), mix(corners[2], corners[3], w.x), w.y);
    let high = mix(mix(corners[4], corners[5], w.x), mix(corners[6], corners[7], w.x), w.y);
    return clamp(0.5 + mix(low, high, w.z) * 0.5773502691896258, 0.0, 1.0);
}

// Cellular (Worley F1) noise: the distance to the nearest of one jittered feature point per cell,
// in cells, clamped to [0, 1]. Zero on a feature point.
fn sc_cellular_noise2(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let c = vec2<i32>(i);
    let f = p - i;
    var nearest = 4.0;
    for (var y = -1; y <= 1; y += 1) {
        for (var x = -1; x <= 1; x += 1) {
            let cell = c + vec2<i32>(x, y);
            let feature = vec2<f32>(f32(x), f32(y)) + vec2<f32>(sc_hash2(cell, seed), sc_hash2(cell, seed ^ 0x68e31da4u));
            nearest = min(nearest, dot(feature - f, feature - f));
        }
    }
    return clamp(sqrt(nearest), 0.0, 1.0);
}

fn sc_cellular_noise3(p: vec3<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let c = vec3<i32>(i);
    let f = p - i;
    var nearest = 4.0;
    for (var z = -1; z <= 1; z += 1) {
        for (var y = -1; y <= 1; y += 1) {
            for (var x = -1; x <= 1; x += 1) {
                let cell = c + vec3<i32>(x, y, z);
                let jitter = vec3<f32>(sc_hash3(cell, seed), sc_hash3(cell, seed ^ 0x68e31da4u), sc_hash3(cell, seed ^ 0x3c6ef372u));
                let feature = vec3<f32>(f32(x), f32(y), f32(z)) + jitter;
                nearest = min(nearest, dot(feature - f, feature - f));
            }
        }
    }
    return clamp(sqrt(nearest), 0.0, 1.0);
}

// One octave of noise of `kind` (`sc_noise_kind_*`) in `dimensions` (2: `p.xy`, otherwise 3).
// `kind` and `dimensions` are shape constants in generated code, so the branches fold away.
fn sc_noise(kind: u32, dimensions: u32, p: vec3<f32>, seed: u32) -> f32 {
    if (dimensions == 2u) {
        if (kind == sc_noise_kind_gradient) { return sc_gradient_noise2(p.xy, seed); }
        if (kind == sc_noise_kind_cellular) { return sc_cellular_noise2(p.xy, seed); }
        return sc_value_noise2(p.xy, seed);
    }
    if (kind == sc_noise_kind_gradient) { return sc_gradient_noise3(p, seed); }
    if (kind == sc_noise_kind_cellular) { return sc_cellular_noise3(p, seed); }
    return sc_value_noise3(p, seed);
}

// Fractional Brownian motion: `octaves` (1–8) of `sc_noise`, each `lacunarity` times finer and
// `gain` times weaker, with its own seed, normalised by the total weight so it stays in [0, 1].
fn sc_fbm(kind: u32, dimensions: u32, p: vec3<f32>, seed: u32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    var sum = 0.0;
    var total = 0.0;
    var amplitude = 1.0;
    var frequency = 1.0;
    for (var k = 0u; k < clamp(octaves, 1u, 8u); k += 1u) {
        sum += amplitude * sc_noise(kind, dimensions, p * frequency, seed + k);
        total += amplitude;
        amplitude *= gain;
        frequency *= lacunarity;
    }
    return select(0.0, sum / total, total > 0.0);
}
