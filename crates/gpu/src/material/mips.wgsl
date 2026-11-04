// The mip passes of material textures (crates/gpu/src/material/textures.rs): each level is the
// area average of the level above, read texel by texel. Odd sides weigh the source cells by their
// overlap (as `texture_sampling::mips` does), so no row or column is dropped. Straight-line apart
// from the bounded loops; no derivatives, no sampler.

@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_mip(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn mip_decode(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn mip_encode(c: vec3<f32>) -> vec3<f32> {
    let l = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - 0.055, l * 12.92, l <= vec3<f32>(0.0031308));
}

// The area average under destination texel `p`; `codes`: the texels hold sRGB codes to decode.
fn mip_area(p: vec4<f32>, codes: bool) -> vec4<f32> {
    let s = vec2<i32>(textureDimensions(src));
    let d = max(s / 2, vec2<i32>(1));
    let o = vec2<i32>(p.xy);
    let x0 = o.x * s.x;
    let x1 = (o.x + 1) * s.x;
    let y0 = o.y * s.y;
    let y1 = (o.y + 1) * s.y;
    var acc = vec4<f32>(0.0);
    for (var sy = y0 / d.y; sy < (y1 + d.y - 1) / d.y; sy += 1) {
        let wy = min((sy + 1) * d.y, y1) - max(sy * d.y, y0);
        for (var sx = x0 / d.x; sx < (x1 + d.x - 1) / d.x; sx += 1) {
            let wx = min((sx + 1) * d.x, x1) - max(sx * d.x, x0);
            let c = textureLoad(src, vec2<i32>(sx, sy), 0);
            let linear = select(c.rgb, mip_decode(c.rgb), codes);
            acc += vec4<f32>(linear, c.a) * f32(wx * wy);
        }
    }
    return acc / f32(s.x * s.y);
}

@fragment
fn fs_mip(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return mip_area(p, false);
}

@fragment
fn fs_mip_srgb(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = mip_area(p, true);
    return vec4<f32>(mip_encode(c.rgb), c.a);
}
