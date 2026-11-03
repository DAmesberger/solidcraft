//! The shared library's noise (`crates/gpu/src/shading/noise.wgsl`), mirrored in f64. The hashes
//! are integer functions and are mirrored bit for bit, including their final `f32` rounding.
//!
//! Mirrors track S's `noise.wgsl` (mg-s, before its first commit). Re-check against it when it
//! lands: `sc_hash2`, `sc_hash3`, the value, gradient and cellular noises, `sc_noise`, `sc_fbm`.

const KIND_GRADIENT: i64 = 1;
const KIND_CELLULAR: i64 = 2;

/// WGSL's `vec2<i32>(floor(p))` and `bitcast<u32>`: the lattice cell as wrapping `u32` bits.
fn cell_bits(x: f64) -> u32 {
    // f32 → i32 conversion saturates in WGSL; so does `as`.
    (x.floor() as i32) as u32
}

/// `sc_hash2`: a lattice cell's hash in [0, 1].
pub fn hash2(cx: i32, cy: i32, seed: u32) -> f64 {
    let mut h = (cx as u32).wrapping_mul(1_597_334_677) ^ (cy as u32).wrapping_mul(3_812_015_801) ^ seed.wrapping_mul(0x9e37_79b9);
    h = h.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277_803_737);
    h ^= h >> 22;
    f64::from(h as f32 / 4_294_967_295.0_f32)
}

/// `sc_hash3`: a lattice cell's hash in [0, 1), periodic over 4096 cells on each axis.
pub fn hash3(c: [i32; 3], seed: u32) -> f64 {
    let [x, y, z] = c.map(|v| (v as u32) & 4095);
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ z.wrapping_mul(0xcb1a_b31f) ^ seed.wrapping_mul(0x9e37_79b9);
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb_352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    f64::from(h >> 8) / 16_777_216.0
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn cell2(p: [f64; 2]) -> ([i32; 2], [f64; 2]) {
    let c = [cell_bits(p[0]) as i32, cell_bits(p[1]) as i32];
    (c, [p[0] - p[0].floor(), p[1] - p[1].floor()])
}

fn cell3(p: [f64; 3]) -> ([i32; 3], [f64; 3]) {
    (p.map(|v| cell_bits(v) as i32), p.map(|v| v - v.floor()))
}

fn add2(c: [i32; 2], x: i32, y: i32) -> (i32, i32) {
    (c[0].wrapping_add(x), c[1].wrapping_add(y))
}

fn add3i(c: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [c[0].wrapping_add(o[0]), c[1].wrapping_add(o[1]), c[2].wrapping_add(o[2])]
}

/// `sc_value_noise2`.
pub fn value2(p: [f64; 2], seed: u32) -> f64 {
    let (c, f) = cell2(p);
    let w = f.map(|f| f * f * (3.0 - 2.0 * f));
    let h = |x, y| {
        let (a, b) = add2(c, x, y);
        hash2(a, b, seed)
    };
    lerp(lerp(h(0, 0), h(1, 0), w[0]), lerp(h(0, 1), h(1, 1), w[0]), w[1])
}

/// `sc_value_noise3`.
pub fn value3(p: [f64; 3], seed: u32) -> f64 {
    let (c, f) = cell3(p);
    let w = f.map(|f| f * f * (3.0 - 2.0 * f));
    let h = |x, y, z| hash3(add3i(c, [x, y, z]), seed);
    let low = lerp(lerp(h(0, 0, 0), h(1, 0, 0), w[0]), lerp(h(0, 1, 0), h(1, 1, 0), w[0]), w[1]);
    let high = lerp(lerp(h(0, 0, 1), h(1, 0, 1), w[0]), lerp(h(0, 1, 1), h(1, 1, 1), w[0]), w[1]);
    lerp(low, high, w[2])
}

const TAU: f64 = std::f64::consts::TAU;

fn gradient2(c: (i32, i32), seed: u32) -> [f64; 2] {
    let angle = TAU * hash2(c.0, c.1, seed);
    [angle.cos(), angle.sin()]
}

fn gradient3(c: [i32; 3], seed: u32) -> [f64; 3] {
    let z = 2.0 * hash3(c, seed) - 1.0;
    let angle = TAU * hash3(c, seed ^ 0x68e3_1da4);
    let r = (1.0 - z * z).max(0.0).sqrt();
    [r * angle.cos(), r * angle.sin(), z]
}

fn quintic(f: f64) -> f64 {
    f * f * f * (f * (f * 6.0 - 15.0) + 10.0)
}

/// `sc_gradient_noise2`: 0.5 on every lattice point.
pub fn gradient_noise2(p: [f64; 2], seed: u32) -> f64 {
    let (c, f) = cell2(p);
    let w = f.map(quintic);
    let corner = |x: i32, y: i32| {
        let g = gradient2(add2(c, x, y), seed);
        g[0] * (f[0] - f64::from(x)) + g[1] * (f[1] - f64::from(y))
    };
    let n = lerp(lerp(corner(0, 0), corner(1, 0), w[0]), lerp(corner(0, 1), corner(1, 1), w[0]), w[1]);
    (0.5 + n * std::f64::consts::FRAC_1_SQRT_2).clamp(0.0, 1.0)
}

/// `sc_gradient_noise3`.
pub fn gradient_noise3(p: [f64; 3], seed: u32) -> f64 {
    let (c, f) = cell3(p);
    let w = f.map(quintic);
    let mut corners = [0.0; 8];
    for (k, corner) in corners.iter_mut().enumerate() {
        let o = [(k & 1) as i32, ((k >> 1) & 1) as i32, ((k >> 2) & 1) as i32];
        let g = gradient3(add3i(c, o), seed);
        *corner = g[0] * (f[0] - f64::from(o[0])) + g[1] * (f[1] - f64::from(o[1])) + g[2] * (f[2] - f64::from(o[2]));
    }
    let low = lerp(lerp(corners[0], corners[1], w[0]), lerp(corners[2], corners[3], w[0]), w[1]);
    let high = lerp(lerp(corners[4], corners[5], w[0]), lerp(corners[6], corners[7], w[0]), w[1]);
    (0.5 + lerp(low, high, w[2]) * 0.577_350_269_189_625_8).clamp(0.0, 1.0)
}

/// `sc_cellular_noise2`: Worley F1 in cells, clamped to [0, 1].
pub fn cellular2(p: [f64; 2], seed: u32) -> f64 {
    let (c, f) = cell2(p);
    let mut nearest: f64 = 4.0;
    for y in -1..=1 {
        for x in -1..=1 {
            let (a, b) = add2(c, x, y);
            let feature = [f64::from(x) + hash2(a, b, seed), f64::from(y) + hash2(a, b, seed ^ 0x68e3_1da4)];
            let d = [feature[0] - f[0], feature[1] - f[1]];
            nearest = nearest.min(d[0] * d[0] + d[1] * d[1]);
        }
    }
    nearest.sqrt().clamp(0.0, 1.0)
}

/// `sc_cellular_noise3`.
pub fn cellular3(p: [f64; 3], seed: u32) -> f64 {
    let (c, f) = cell3(p);
    let mut nearest: f64 = 4.0;
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                let cell = add3i(c, [x, y, z]);
                let jitter = [hash3(cell, seed), hash3(cell, seed ^ 0x68e3_1da4), hash3(cell, seed ^ 0x3c6e_f372)];
                let d = [f64::from(x) + jitter[0] - f[0], f64::from(y) + jitter[1] - f[1], f64::from(z) + jitter[2] - f[2]];
                nearest = nearest.min(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            }
        }
    }
    nearest.sqrt().clamp(0.0, 1.0)
}

/// `sc_noise`: one octave of `kind` (the node's choice index) in 2 (`p.xy`) or 3 dimensions.
pub fn noise(kind: i64, dimensions: i64, p: [f64; 3], seed: u32) -> f64 {
    if dimensions == 2 {
        let q = [p[0], p[1]];
        return match kind {
            KIND_GRADIENT => gradient_noise2(q, seed),
            KIND_CELLULAR => cellular2(q, seed),
            _ => value2(q, seed),
        };
    }
    match kind {
        KIND_GRADIENT => gradient_noise3(p, seed),
        KIND_CELLULAR => cellular3(p, seed),
        _ => value3(p, seed),
    }
}

/// `sc_fbm`: `octaves` (1–8) of [`noise`], each `lacunarity` times finer and `gain` times weaker
/// with seed `seed + k`, normalised by the total weight.
pub fn fbm(kind: i64, dimensions: i64, p: [f64; 3], seed: u32, octaves: i64, lacunarity: f64, gain: f64) -> f64 {
    let (mut sum, mut total, mut amplitude, mut frequency) = (0.0, 0.0, 1.0, 1.0);
    for k in 0..octaves.clamp(1, 8) {
        sum += amplitude * noise(kind, dimensions, p.map(|c| c * frequency), seed.wrapping_add(k as u32));
        total += amplitude;
        amplitude *= gain;
        frequency *= lacunarity;
    }
    if total > 0.0 { sum / total } else { 0.0 }
}
