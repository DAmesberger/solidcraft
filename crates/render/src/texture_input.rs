//! Bounded texture admission, independent of documents, files, schedulers and GPU handles.
//! This utility does not advertise texture/PBR support or define a persistent pack schema.
//!
//! The limits are safety bounds against hostile input, never quality caps
//! (docs/material-graph.md §4.3): a renderer chooses the resident size of what is admitted here
//! ([`DecodedTexture::fit_within`] is its decode-time downsampling).
//!
//! - Size: 1–[`MAX_TEXTURE_EDGE`] px a side; encoded: at most [`MAX_TEXTURE_BYTES`].
//! - Decoded pixels: RGBA8 at 4 B/px, or 16-bit grey at 2 B/px for [`MapRole::Height`]. A
//!   design's textures together stay within [`MAX_DESIGN_DECODED_BYTES`], checked from the
//!   headers before any decode ([`DecodeBudget`]).
//! - Codec guard: each decode passes the header's decoded size plus [`CODEC_SCRATCH_BYTES`] as
//!   the codec's allocation limit, so a header that lies about its size cannot make the codec
//!   allocate past it.
use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::{io::Cursor, sync::Arc};

pub mod normalization;
pub(crate) mod png_metadata;

/// Encoded bytes of one texture (the design container's per-part cap).
pub const MAX_TEXTURE_BYTES: usize = 256 * 1024 * 1024;
/// Pixels a side: the largest 2D texture any current desktop GPU holds in one piece.
pub const MAX_TEXTURE_EDGE: u32 = 16384;
/// Decoded bytes of all of one design's textures together.
pub const MAX_DESIGN_DECODED_BYTES: u64 = 1024 * 1024 * 1024;
/// Codec working memory allowed on top of an image's decoded size.
pub const CODEC_SCRATCH_BYTES: u64 = 64 * 1024 * 1024;
/// Mip levels of a texture of [`MAX_TEXTURE_EDGE`] a side, through 1 × 1.
pub const MAX_MIP_LEVELS: usize = MAX_TEXTURE_EDGE.ilog2() as usize + 1;
pub(crate) const MAX_CODEC_BYTES: u64 = CODEC_SCRATCH_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapRole {
    BaseColor,
    NormalOpenGl,
    Roughness,
    Metallic,
    AmbientOcclusion,
    /// Grey height data, 8- or 16-bit, decoded to 16 bits ([`PixelLayout::Gray16`]).
    Height,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpretation {
    Srgb,
    Data,
}
impl MapRole {
    pub fn interpretation(self) -> Interpretation {
        if self == Self::BaseColor { Interpretation::Srgb } else { Interpretation::Data }
    }
    /// How this role's pixels are decoded.
    pub fn layout(self) -> PixelLayout {
        if self == Self::Height { PixelLayout::Gray16 } else { PixelLayout::Rgba8 }
    }
}

/// The decoded pixel layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelLayout {
    /// Four 8-bit channels, as encoded (sRGB or data).
    Rgba8,
    /// One 16-bit channel, little-endian (height data).
    Gray16,
}
impl PixelLayout {
    pub fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgba8 => 4,
            Self::Gray16 => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureError(pub String);
impl std::fmt::Display for TextureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for TextureError {}
type Result<T> = std::result::Result<T, TextureError>;
fn invalid(message: impl Into<String>) -> TextureError {
    TextureError(message.into())
}

#[derive(Clone, Debug, Serialize)]
pub struct TextureInfo {
    pub width: u32,
    pub height: u32,
    pub encoded_bytes: usize,
    /// The image as RGBA8 (4 B/px), whatever its layout.
    pub rgba_bytes: u64,
    /// The decoded pixels ([`PixelLayout::bytes_per_pixel`]): what [`DecodeBudget`] counts.
    pub decoded_bytes: u64,
    pub layout: PixelLayout,
    pub role: MapRole,
    pub interpretation: Interpretation,
}

/// The decoded-pixel budget of one design ([`MAX_DESIGN_DECODED_BYTES`]), charged from the
/// headers of its textures before any of them decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeBudget {
    remaining: u64,
}
impl Default for DecodeBudget {
    fn default() -> Self {
        Self::design()
    }
}
impl DecodeBudget {
    /// One design's budget.
    pub fn design() -> Self {
        Self { remaining: MAX_DESIGN_DECODED_BYTES }
    }
    /// A smaller budget (never more than a design's).
    pub fn with_bytes(bytes: u64) -> Self {
        Self { remaining: bytes.min(MAX_DESIGN_DECODED_BYTES) }
    }
    pub fn remaining(&self) -> u64 {
        self.remaining
    }
    /// Charge `info`'s decoded size, or refuse it (leaving the budget unchanged).
    pub fn admit(&mut self, info: &TextureInfo) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(info.decoded_bytes)
            .ok_or_else(|| invalid(format!("the design's textures decode to more than {} MiB", MAX_DESIGN_DECODED_BYTES >> 20)))?;
        Ok(())
    }
    /// Give back what [`Self::admit`] charged for `info` (its texture was dropped).
    pub fn release(&mut self, info: &TextureInfo) {
        self.remaining = self.remaining.saturating_add(info.decoded_bytes).min(MAX_DESIGN_DECODED_BYTES);
    }
}

/// An explicit base-color role means normalized sRGB samples. Reject conflicting
/// encoded declarations; data roles preserve their numerical channels unchanged.
pub fn validate_color_declarations(bytes: &[u8], role: MapRole) -> Result<()> {
    if role == MapRole::BaseColor && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png_metadata::validate_srgb_declarations(bytes)?;
    }
    Ok(())
}

/// Header-admitted source. Full decoding must succeed before publishing pixels.
#[derive(Clone)]
pub struct TextureSource {
    bytes: Arc<Vec<u8>>,
    format: ImageFormat,
    info: TextureInfo,
}
fn reader(bytes: &[u8], format: ImageFormat, max_alloc: u64) -> ImageReader<Cursor<&[u8]>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_TEXTURE_EDGE);
    limits.max_image_height = Some(MAX_TEXTURE_EDGE);
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);
    reader
}
fn guarded<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|_| invalid("texture decoder failed"))?
}
fn admitted_format(bytes: &[u8], capacity: usize) -> Result<ImageFormat> {
    if bytes.is_empty() || bytes.len() > MAX_TEXTURE_BYTES || capacity > MAX_TEXTURE_BYTES {
        return Err(invalid(format!("texture input must contain at most {} MiB", MAX_TEXTURE_BYTES >> 20)));
    }
    let format = image::guess_format(bytes).map_err(|_| invalid("texture must be PNG or JPEG"))?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err(invalid("texture must be PNG or JPEG; HDR uses the Radiance input utility"));
    }
    // Whole-image codecs may return pixels before noticing a missing trailer. This
    // marker check rejects that truncation; it is not a general container validator.
    let complete = match format {
        ImageFormat::Png => bytes.ends_with(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"),
        ImageFormat::Jpeg => bytes.ends_with(b"\xff\xd9"),
        _ => false,
    };
    if !complete {
        return Err(invalid("texture is missing its final terminator"));
    }
    Ok(format)
}
fn admitted_color(role: MapRole, color: image::ColorType) -> Result<()> {
    use image::ColorType as C;
    let ok = match role.layout() {
        PixelLayout::Rgba8 => matches!(color, C::L8 | C::La8 | C::Rgb8 | C::Rgba8),
        PixelLayout::Gray16 => matches!(color, C::L8 | C::L16),
    };
    match (ok, role.layout()) {
        (true, _) => Ok(()),
        (false, PixelLayout::Rgba8) => Err(invalid("only 8-bit texture channels are supported")),
        (false, PixelLayout::Gray16) => Err(invalid("a height map is 8- or 16-bit grey")),
    }
}
/// A zero-filled buffer, or an error instead of an allocation failure.
fn zeroed(length: u64) -> Result<Vec<u8>> {
    let length = usize::try_from(length).map_err(|_| invalid("texture pixel storage overflow"))?;
    let mut values = Vec::new();
    values.try_reserve_exact(length).map_err(|_| invalid("cannot allocate texture pixels"))?;
    values.resize(length, 0);
    Ok(values)
}
impl TextureSource {
    #[cfg(test)]
    pub(crate) fn shares_encoded_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes)
    }
    pub fn new(bytes: Vec<u8>, role: MapRole) -> Result<Self> {
        let format = admitted_format(&bytes, bytes.capacity())?;
        if format == ImageFormat::Png {
            png_metadata::validate(&bytes)?;
        }
        let (width, height) = guarded(|| {
            let mut decoder = reader(&bytes, format, CODEC_SCRATCH_BYTES).into_decoder().map_err(|e| invalid(e.to_string()))?;
            admitted_color(role, decoder.color_type())?;
            if decoder.icc_profile().map_err(|e| invalid(e.to_string()))?.is_some() {
                return Err(invalid("embedded ICC profiles need explicit color conversion before admission"));
            }
            if decoder.orientation().map_err(|e| invalid(e.to_string()))? != image::metadata::Orientation::NoTransforms {
                return Err(invalid("texture orientation must be normalized before admission"));
            }
            Ok(decoder.dimensions())
        })?;
        if width == 0 || height == 0 || width > MAX_TEXTURE_EDGE || height > MAX_TEXTURE_EDGE {
            return Err(invalid(format!("texture dimensions exceed {MAX_TEXTURE_EDGE}x{MAX_TEXTURE_EDGE}")));
        }
        let pixels = u64::from(width) * u64::from(height);
        let layout = role.layout();
        let info = TextureInfo {
            width,
            height,
            encoded_bytes: bytes.len(),
            rgba_bytes: pixels * 4,
            decoded_bytes: pixels * layout.bytes_per_pixel(),
            layout,
            role,
            interpretation: role.interpretation(),
        };
        Ok(Self { bytes: Arc::new(bytes), format, info })
    }
    /// Admit a portable source with strict PNG metadata and color declarations
    /// before opening its codec. Legacy runtime inputs keep `new`'s admission.
    pub fn new_portable(bytes: Vec<u8>, role: MapRole) -> Result<Self> {
        if admitted_format(&bytes, bytes.capacity())? == ImageFormat::Png {
            png_metadata::validate(&bytes)?;
            validate_color_declarations(&bytes, role)?;
        }
        Self::new(bytes, role)
    }
    pub fn info(&self) -> &TextureInfo {
        &self.info
    }
    /// Complete static-source metadata admission before portable publication.
    /// This bounded scan belongs on the caller's existing import worker. It does
    /// not replace pixel decoding or the cooperative image worker's own checks.
    pub fn validate_static_source(&self) -> Result<()> {
        if self.format == ImageFormat::Png {
            let mut metadata = png_metadata::PngMetadata::new(true, true).with_role(self.info.role);
            while !metadata.step(&self.bytes)? {}
        }
        Ok(())
    }
    /// Share admitted encoded ownership with the cooperative native row worker.
    #[cfg(feature = "raytrace-cpu")]
    pub(crate) fn png_encoded(&self) -> Result<Arc<Vec<u8>>> {
        if self.format != ImageFormat::Png || self.bytes.get(24).copied() != Some(8) || self.bytes.get(28).copied() != Some(0) {
            return Err(invalid("CPU row preparation supports noninterlaced 8-bit PNG only"));
        }
        Ok(self.bytes.clone())
    }
    #[cfg(all(test, feature = "raytrace-cpu"))]
    pub(crate) fn encoded_weak(&self) -> std::sync::Weak<Vec<u8>> {
        Arc::downgrade(&self.bytes)
    }
    /// Bounded whole-image codec work, intended for a caller-owned worker. Cancellation is
    /// checked at the codec boundary and between rows of the layout conversion, not during
    /// PNG/JPEG decoding. No partial image escapes. Pixels are decoded straight into their
    /// final buffer, so the peak is the decoded size plus the codec's own scratch.
    pub fn decode(&self, cancelled: impl Fn() -> bool) -> Result<DecodedTexture> {
        if cancelled() {
            return Err(invalid("texture decoding cancelled"));
        }
        let info = &self.info;
        let guard = info.decoded_bytes.saturating_add(CODEC_SCRATCH_BYTES);
        let mut pixels = zeroed(info.decoded_bytes)?;
        let color = guarded(|| {
            let decoder = reader(&self.bytes, self.format, guard).into_decoder().map_err(|e| invalid(e.to_string()))?;
            if decoder.dimensions() != (info.width, info.height) {
                return Err(invalid("decoded texture dimensions changed"));
            }
            let color = decoder.color_type();
            admitted_color(info.role, color)?;
            let total = usize::try_from(decoder.total_bytes()).map_err(|_| invalid("texture pixel storage overflow"))?;
            let target = pixels.get_mut(..total).ok_or_else(|| invalid("decoded texture is larger than its header"))?;
            decoder.read_image(target).map_err(|e| invalid(e.to_string()))?;
            Ok(color)
        })?;
        if cancelled() {
            return Err(invalid("texture decoding cancelled"));
        }
        expand(&mut pixels, color, info.layout, &cancelled)?;
        if cancelled() {
            return Err(invalid("texture decoding cancelled"));
        }
        Ok(DecodedTexture { info: info.clone(), pixels })
    }
}

/// Widen the codec's samples at the front of `pixels` to `layout`, in place, from the back: each
/// output pixel is at least as wide as its input, so nothing unread is overwritten.
fn expand(pixels: &mut [u8], color: image::ColorType, layout: PixelLayout, cancelled: &impl Fn() -> bool) -> Result<()> {
    use image::ColorType as C;
    let check = |i: usize| if i.is_multiple_of(65536) && cancelled() { Err(invalid("texture decoding cancelled")) } else { Ok(()) };
    match (layout, color) {
        (PixelLayout::Rgba8, C::Rgba8) => Ok(()),
        (PixelLayout::Gray16, C::L16) => {
            // Native-endian samples as the codec wrote them; the layout is little-endian.
            if cfg!(target_endian = "big") {
                for pair in pixels.as_chunks_mut::<2>().0 {
                    pair.swap(0, 1);
                }
            }
            Ok(())
        }
        (PixelLayout::Gray16, C::L8) => {
            for i in (0..pixels.len() / 2).rev() {
                check(i)?;
                let v = u16::from(pixels.get(i).copied().unwrap_or(0)) * 257;
                if let Some(out) = pixels.get_mut(2 * i..2 * i + 2) {
                    out.copy_from_slice(&v.to_le_bytes());
                }
            }
            Ok(())
        }
        (PixelLayout::Rgba8, C::L8 | C::La8 | C::Rgb8) => {
            let n = color.bytes_per_pixel() as usize;
            for i in (0..pixels.len() / 4).rev() {
                check(i)?;
                let mut s = [0u8; 3];
                if let (Some(src), Some(dst)) = (pixels.get(i * n..i * n + n), s.get_mut(..n)) {
                    dst.copy_from_slice(src);
                }
                let rgba = match n {
                    1 => [s[0], s[0], s[0], 255],
                    2 => [s[0], s[0], s[0], s[1]],
                    _ => [s[0], s[1], s[2], 255],
                };
                if let Some(out) = pixels.get_mut(4 * i..4 * i + 4) {
                    out.copy_from_slice(&rgba);
                }
            }
            Ok(())
        }
        _ => Err(invalid("unsupported decoded channel layout")),
    }
}

/// Completed pixels retain encoded channel values. sRGB conversion, normal basis handling
/// and sampling belong to the eventual renderer adapter, not this input utility.
pub struct DecodedTexture {
    info: TextureInfo,
    pixels: Vec<u8>,
}
impl DecodedTexture {
    pub fn info(&self) -> &TextureInfo {
        &self.info
    }
    /// RGBA8 pixels, row 0 at the top; empty for [`PixelLayout::Gray16`] (see [`Self::pixels`]).
    pub fn rgba(&self) -> &[u8] {
        if self.info.layout == PixelLayout::Rgba8 { &self.pixels } else { &[] }
    }
    /// The pixels in [`TextureInfo::layout`], row 0 at the top.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    /// Take the pixels.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// The next floor-halved level (at least 1 px a side), area-filtered as the GPU's mips are:
    /// sRGB colour averaged in linear light, data and alpha as stored. Odd sides weigh the
    /// source cells by their overlap, so no row or column is dropped. Data preparation for a
    /// renderer's resident size; `None` at 1 × 1.
    pub fn halve(&self, cancelled: impl Fn() -> bool) -> Result<Option<DecodedTexture>> {
        let (sw, sh) = (self.info.width as usize, self.info.height as usize);
        if sw <= 1 && sh <= 1 {
            return Ok(None);
        }
        let (dw, dh) = ((sw / 2).max(1), (sh / 2).max(1));
        let layout = self.info.layout;
        let bpp = layout.bytes_per_pixel() as usize;
        let mut out = zeroed((dw * dh * bpp) as u64)?;
        let srgb = self.info.interpretation == Interpretation::Srgb;
        let decode: [f32; 256] = std::array::from_fn(|c| if srgb { solidcraft_color::srgb::decode8(c as u8) as f32 } else { c as f32 / 255.0 });
        let channels = if layout == PixelLayout::Rgba8 { 4 } else { 1 };
        let read = |x: usize, y: usize, c: usize| -> f32 {
            let i = (y * sw + x) * bpp;
            match layout {
                PixelLayout::Rgba8 => {
                    let v = self.pixels.get(i + c).copied().unwrap_or(0);
                    if c < 3 { decode[usize::from(v)] } else { f32::from(v) / 255.0 }
                }
                PixelLayout::Gray16 => {
                    let b = [self.pixels.get(i).copied().unwrap_or(0), self.pixels.get(i + 1).copied().unwrap_or(0)];
                    f32::from(u16::from_le_bytes(b)) / 65535.0
                }
            }
        };
        for y in 0..dh {
            if cancelled() {
                return Err(invalid("texture downsampling cancelled"));
            }
            let (y0, y1) = (y * sh, (y + 1) * sh);
            for x in 0..dw {
                let (x0, x1) = (x * sw, (x + 1) * sw);
                let mut acc = [0f64; 4];
                for sy in y0 / dh..y1.div_ceil(dh) {
                    let wy = ((sy + 1) * dh).min(y1) - (sy * dh).max(y0);
                    for sx in x0 / dw..x1.div_ceil(dw) {
                        let wx = ((sx + 1) * dw).min(x1) - (sx * dw).max(x0);
                        let w = (wx * wy) as f64;
                        for (c, a) in acc.iter_mut().enumerate().take(channels) {
                            *a += f64::from(read(sx, sy, c)) * w;
                        }
                    }
                }
                let area = (sw * sh) as f64;
                let o = (y * dw + x) * bpp;
                match layout {
                    PixelLayout::Rgba8 => {
                        for (c, a) in acc.iter().enumerate() {
                            let v = (a / area).clamp(0.0, 1.0);
                            let code = if c < 3 && srgb { solidcraft_color::srgb::encode8(v) } else { (v * 255.0).round() as u8 };
                            if let Some(slot) = out.get_mut(o + c) {
                                *slot = code;
                            }
                        }
                    }
                    PixelLayout::Gray16 => {
                        let v = ((acc[0] / area).clamp(0.0, 1.0) * 65535.0).round() as u16;
                        if let Some(slot) = out.get_mut(o..o + 2) {
                            slot.copy_from_slice(&v.to_le_bytes());
                        }
                    }
                }
            }
        }
        let (width, height) = (dw as u32, dh as u32);
        let pixels = u64::from(width) * u64::from(height);
        let info = TextureInfo { width, height, rgba_bytes: pixels * 4, decoded_bytes: pixels * bpp as u64, ..self.info.clone() };
        Ok(Some(DecodedTexture { info, pixels: out }))
    }

    /// Halve until both sides are at most `max_edge` (the renderer's resident clamp, §4.3).
    /// Returns the texture and how many levels were dropped.
    pub fn fit_within(self, max_edge: u32, cancelled: impl Fn() -> bool) -> Result<(DecodedTexture, u32)> {
        let max_edge = max_edge.max(1);
        let mut texture = self;
        let mut dropped = 0;
        while texture.info.width > max_edge || texture.info.height > max_edge {
            match texture.halve(&cancelled)? {
                Some(next) => texture = next,
                None => break,
            }
            dropped += 1;
        }
        Ok((texture, dropped))
    }
}

/// The role an `image` binding reads `bytes` as: sRGB colour, else 16-bit (or 8-bit) grey height
/// when the PNG header says 16-bit grey, else generic 8-bit data. The role follows from the node's
/// use (docs/material-graph.md §4.5).
pub fn role_of(bytes: &[u8], srgb: bool) -> MapRole {
    // PNG: the IHDR's bit depth is byte 24 and its colour type byte 25 (0: grey).
    let grey16 = bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(24) == Some(&16) && bytes.get(25) == Some(&0);
    match (srgb, grey16) {
        (true, _) => MapRole::BaseColor,
        (false, true) => MapRole::Height,
        (false, false) => MapRole::Roughness,
    }
}

#[cfg(test)]
mod tests;
