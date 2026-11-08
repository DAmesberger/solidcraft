use super::*;

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([64, 128, 192, 255]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}
#[test]
fn texture_input_preserves_channels_and_explicit_map_interpretation() {
    for role in [MapRole::BaseColor, MapRole::NormalOpenGl, MapRole::Roughness, MapRole::Metallic, MapRole::AmbientOcclusion] {
        let source = TextureSource::new(png(3, 2), role).unwrap();
        let decoded = source.decode(|| false).unwrap();
        assert_eq!((decoded.info().width, decoded.info().height, decoded.info().rgba_bytes), (3, 2, 24));
        assert_eq!(decoded.info().interpretation, if role == MapRole::BaseColor { Interpretation::Srgb } else { Interpretation::Data });
        assert!(decoded.rgba().as_chunks::<4>().0.iter().all(|p| *p == [64, 128, 192, 255]));
    }
}
#[test]
fn texture_input_rejects_dimensions_formats_and_corrupt_pixels() {
    assert!(TextureSource::new(png(MAX_TEXTURE_EDGE + 1, 1), MapRole::BaseColor).is_err());
    assert!(TextureSource::new(Vec::new(), MapRole::BaseColor).is_err());
    assert!(TextureSource::new(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\nabcdefgh".to_vec(), MapRole::BaseColor).is_err());
    let bytes = png(3, 2);
    for n in 0..bytes.len() {
        if let Ok(source) = TextureSource::new(bytes.get(..n).unwrap().to_vec(), MapRole::BaseColor) {
            assert!(source.decode(|| false).is_err(), "truncated PNG accepted at {n}");
        }
    }
}
#[test]
fn texture_input_cancellation_never_publishes_partial_pixels() {
    let source = TextureSource::new(png(3, 2), MapRole::NormalOpenGl).unwrap();
    assert!(source.decode(|| true).is_err());
    let calls = std::cell::Cell::new(0);
    assert!(
        source
            .decode(|| {
                calls.set(calls.get() + 1);
                calls.get() == 2
            })
            .is_err()
    );
    assert!(source.clone().decode(|| false).is_ok());
}
#[test]
fn texture_input_jpeg_is_supported_without_hdr_feature_confusion() {
    let image = image::RgbImage::from_pixel(2, 2, image::Rgb([64, 128, 192]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Jpeg).unwrap();
    let source = TextureSource::new(out.into_inner(), MapRole::BaseColor).unwrap();
    assert_eq!(source.decode(|| false).unwrap().rgba().len(), 16);
}

#[test]
fn texture_input_caps_encoded_storage_and_refuses_precision_loss() {
    let mut bytes = Vec::with_capacity(MAX_TEXTURE_BYTES + 1);
    bytes.extend(png(1, 1));
    assert!(TextureSource::new(bytes, MapRole::Roughness).is_err());
    assert!(TextureSource::new(vec![0; MAX_TEXTURE_BYTES + 1], MapRole::BaseColor).is_err());
    let image = image::DynamicImage::ImageLuma16(image::ImageBuffer::from_pixel(1, 1, image::Luma([32768u16])));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    assert!(TextureSource::new(out.into_inner(), MapRole::NormalOpenGl).is_err());
}

fn encode(image: image::DynamicImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// `png` with its header claiming `width` × `height` (the CRC fixed, so only the size lies).
fn lying(mut png: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
    png[16..20].copy_from_slice(&width.to_be_bytes());
    png[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    png
}

#[test]
fn the_limits_are_the_files_safety_limits() {
    use crate::material_graph::limits;
    assert_eq!(MAX_TEXTURE_EDGE, limits::TEXTURE_SIDE);
    assert_eq!(MAX_TEXTURE_BYTES, limits::TEXTURE_BYTES);
    assert_eq!(MAX_DESIGN_DECODED_BYTES, limits::DECODED_TEXTURE_BYTES as u64);
    assert_eq!(MAX_MIP_LEVELS, 15);
    // Exactly one maximum-size RGBA8 image fits a design's budget.
    assert_eq!(u64::from(MAX_TEXTURE_EDGE).pow(2) * 4, MAX_DESIGN_DECODED_BYTES);
}

#[test]
fn texture_input_admits_the_largest_edge_and_refuses_one_more() {
    let wide = encode(image::DynamicImage::ImageLuma8(image::GrayImage::new(MAX_TEXTURE_EDGE, 1)));
    let source = TextureSource::new(wide, MapRole::Roughness).unwrap();
    assert_eq!((source.info().width, source.info().decoded_bytes), (MAX_TEXTURE_EDGE, u64::from(MAX_TEXTURE_EDGE) * 4));
    assert_eq!(source.decode(|| false).unwrap().rgba().len(), MAX_TEXTURE_EDGE as usize * 4);
    let wider = encode(image::DynamicImage::ImageLuma8(image::GrayImage::new(MAX_TEXTURE_EDGE + 1, 1)));
    assert!(TextureSource::new(wider, MapRole::Roughness).is_err());
    // A header that claims more than the edge is refused before any decode.
    assert!(TextureSource::new(lying(png(1, 1), MAX_TEXTURE_EDGE + 1, 1), MapRole::BaseColor).is_err());
    assert!(TextureSource::new(lying(png(1, 1), 1, u32::MAX), MapRole::BaseColor).is_err());
}

#[test]
fn a_header_that_lies_about_its_size_fails_to_decode_within_its_guard() {
    // The header claims 2048 × 2048: admitted by size, but the data holds one pixel.
    let source = TextureSource::new(lying(png(1, 1), 2048, 2048), MapRole::BaseColor).unwrap();
    assert_eq!(source.info().decoded_bytes, 2048 * 2048 * 4);
    assert!(source.decode(|| false).is_err());
    let height = encode(image::DynamicImage::ImageLuma16(image::ImageBuffer::from_pixel(1, 1, image::Luma([7u16]))));
    let source = TextureSource::new(lying(height, 4096, 3), MapRole::Height).unwrap();
    assert!(source.decode(|| false).is_err());
}

#[test]
fn every_8_bit_layout_widens_to_rgba8() {
    let grey = encode(image::DynamicImage::ImageLuma8(image::ImageBuffer::from_fn(3, 2, |x, y| image::Luma([(10 * x + 100 * y) as u8]))));
    let la = encode(image::DynamicImage::ImageLumaA8(image::ImageBuffer::from_fn(3, 2, |x, y| image::LumaA([(10 * x) as u8, (50 + y) as u8]))));
    let rgb = encode(image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(3, 2, |x, y| image::Rgb([x as u8, y as u8, 200]))));
    let decoded = |bytes: Vec<u8>| TextureSource::new(bytes, MapRole::Metallic).unwrap().decode(|| false).unwrap();
    let g = decoded(grey);
    let l = decoded(la);
    let c = decoded(rgb);
    for (i, ((g, l), c)) in g.rgba().as_chunks::<4>().0.iter().zip(l.rgba().as_chunks::<4>().0).zip(c.rgba().as_chunks::<4>().0).enumerate() {
        let (x, y) = ((i % 3) as u8, (i / 3) as u8);
        assert_eq!(*g, [10 * x + 100 * y, 10 * x + 100 * y, 10 * x + 100 * y, 255]);
        assert_eq!(*l, [10 * x, 10 * x, 10 * x, 50 + y]);
        assert_eq!(*c, [x, y, 200, 255]);
    }
}

#[test]
fn height_maps_keep_16_bits_and_widen_8_bit_grey() {
    let values = [0u16, 1, 0x1234, 0xfffe, 0xffff, 40000];
    let deep = encode(image::DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(3, 2, |x, y| image::Luma([values[(y * 3 + x) as usize]]))));
    let source = TextureSource::new(deep.clone(), MapRole::Height).unwrap();
    assert_eq!((source.info().layout, source.info().decoded_bytes, source.info().interpretation), (PixelLayout::Gray16, 12, Interpretation::Data));
    let decoded = source.decode(|| false).unwrap();
    assert!(decoded.rgba().is_empty(), "a height map has no RGBA8 pixels");
    let got: Vec<u16> = decoded.pixels().as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
    assert_eq!(got, values);
    let shallow = encode(image::DynamicImage::ImageLuma8(image::ImageBuffer::from_fn(2, 1, |x, _| image::Luma([[0u8, 200][x as usize]]))));
    let decoded = TextureSource::new(shallow, MapRole::Height).unwrap().decode(|| false).unwrap();
    assert_eq!(decoded.pixels(), [0, 0, 0xc8, 0xc8]);
    // Height is grey; colour, and 16 bits for anything but height, are refused.
    assert!(TextureSource::new(png(1, 1), MapRole::Height).is_err());
    assert!(TextureSource::new(deep, MapRole::Roughness).is_err());
    assert_eq!(serde_json::to_value(MapRole::Height).unwrap(), "height");
}

#[test]
fn a_designs_textures_share_one_decoded_budget() {
    let info = |width: u32, layout: PixelLayout| TextureInfo {
        width,
        height: width,
        encoded_bytes: 1,
        rgba_bytes: u64::from(width).pow(2) * 4,
        decoded_bytes: u64::from(width).pow(2) * layout.bytes_per_pixel(),
        layout,
        role: MapRole::BaseColor,
        interpretation: Interpretation::Srgb,
    };
    let mut budget = DecodeBudget::design();
    let largest = info(MAX_TEXTURE_EDGE, PixelLayout::Rgba8);
    budget.admit(&largest).unwrap();
    assert_eq!(budget.remaining(), 0);
    assert!(budget.admit(&info(1, PixelLayout::Gray16)).is_err());
    budget.release(&largest);
    for _ in 0..16 {
        budget.admit(&info(4096, PixelLayout::Rgba8)).unwrap();
    }
    assert!(budget.admit(&info(1, PixelLayout::Rgba8)).is_err(), "sixteen 4096² RGBA8 fill it");
    let mut heights = DecodeBudget::design();
    for _ in 0..32 {
        heights.admit(&info(4096, PixelLayout::Gray16)).unwrap();
    }
    assert_eq!(heights.remaining(), 0);
    assert_eq!(DecodeBudget::with_bytes(u64::MAX), DecodeBudget::design());
}

fn decoded(width: u32, height: u32, role: MapRole, pixel: impl Fn(u32, u32) -> [u8; 4]) -> DecodedTexture {
    let image = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
    TextureSource::new(encode(image::DynamicImage::ImageRgba8(image)), role).unwrap().decode(|| false).unwrap()
}

#[test]
fn halving_averages_srgb_in_linear_light_and_data_as_stored() {
    let checker = |x: u32, y: u32| if (x + y).is_multiple_of(2) { [0, 0, 0, 0] } else { [255, 255, 255, 255] };
    let colour = decoded(4, 2, MapRole::BaseColor, checker).halve(|| false).unwrap().unwrap();
    assert_eq!((colour.info().width, colour.info().height, colour.info().decoded_bytes), (2, 1, 8));
    // Half black, half white is linear 0.5: sRGB code 188. Alpha averages as stored.
    let half = solidcraft_color::srgb::encode8(0.5);
    assert_eq!(half, 188);
    assert_eq!(colour.rgba(), [half, half, half, 128, half, half, half, 128]);
    let data = decoded(4, 2, MapRole::Roughness, checker).halve(|| false).unwrap().unwrap();
    assert_eq!(data.rgba(), [128; 8]);
}

#[test]
fn halving_an_odd_side_weighs_every_source_cell() {
    // 3 × 1 → 1 × 1: each source column covers a third.
    let data = decoded(3, 1, MapRole::Metallic, |x, _| [[0, 30, 90][x as usize], 0, 0, 255]).halve(|| false).unwrap().unwrap();
    assert_eq!(data.rgba(), [40, 0, 0, 255]);
    // 5 × 1 → 2 × 1: cells weigh 2, 2, 1 | 1, 2, 2 fifths… of a half: (0·2 + 50·2 + 100·1)/5 = 40.
    let data = decoded(5, 1, MapRole::Metallic, |x, _| [(50 * x) as u8, 0, 0, 255]).halve(|| false).unwrap().unwrap();
    assert_eq!(data.rgba(), [40, 0, 0, 255, 160, 0, 0, 255]);
    let mut texture = decoded(1, 1, MapRole::Metallic, |_, _| [1, 2, 3, 4]);
    assert!(texture.halve(|| false).unwrap().is_none());
    texture = decoded(1, 2, MapRole::Metallic, |_, y| [100 * y as u8, 0, 0, 255]);
    assert_eq!(texture.halve(|| false).unwrap().unwrap().rgba(), [50, 0, 0, 255]);
}

#[test]
fn heights_halve_at_16_bits() {
    let deep = encode(image::DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(2, 2, |x, y| {
        image::Luma([[1000u16, 2001, 3000, 4000][(y * 2 + x) as usize]])
    })));
    let half = TextureSource::new(deep, MapRole::Height).unwrap().decode(|| false).unwrap().halve(|| false).unwrap().unwrap();
    assert_eq!(half.pixels(), 2500u16.to_le_bytes());
    assert_eq!(half.info().decoded_bytes, 2);
}

#[test]
fn fitting_within_an_edge_halves_by_powers_of_two() {
    let texture = decoded(16, 5, MapRole::Roughness, |x, _| [(x * 16) as u8, 0, 0, 255]);
    let (fitted, dropped) = texture.fit_within(4, || false).unwrap();
    assert_eq!((fitted.info().width, fitted.info().height, dropped), (4, 1, 2));
    let texture = decoded(3, 3, MapRole::Roughness, |_, _| [9, 9, 9, 9]);
    let (fitted, dropped) = texture.fit_within(4, || false).unwrap();
    assert_eq!((fitted.info().width, dropped, fitted.rgba()), (3, 0, [9u8; 36].as_slice()));
    assert!(decoded(4, 4, MapRole::Roughness, |_, _| [0; 4]).fit_within(1, || true).is_err(), "cancellable");
}

fn metadata_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    // Independent bitwise IEEE CRC; production uses its fixed lookup table.
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(data);
    let mut crc = u32::MAX;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 { crc >> 1 } else { (crc >> 1) ^ 0xedb88320 };
        }
    }
    out.extend((!crc).to_be_bytes());
    out
}
fn normalized_exif(entries: u16, orientation: u16) -> Vec<u8> {
    let mut data = b"II".to_vec();
    data.extend(42_u16.to_le_bytes());
    data.extend(8_u32.to_le_bytes());
    data.extend(entries.to_le_bytes());
    for i in 0..entries {
        data.extend((if i == 0 { 0x112_u16 } else { 0xc000 + i }).to_le_bytes());
        data.extend(3_u16.to_le_bytes());
        data.extend(1_u32.to_le_bytes());
        data.extend((if i == 0 { orientation } else { i }).to_le_bytes());
        data.extend(0_u16.to_le_bytes());
    }
    data.extend(0_u32.to_le_bytes());
    data
}
fn insert_metadata(mut image: Vec<u8>, kind: &[u8; 4], data: &[u8], before_data: bool) -> Vec<u8> {
    let at = if before_data { 33 } else { image.len() - 12 };
    image.splice(at..at, metadata_chunk(kind, data));
    image
}
#[test]
fn png_metadata_policy_refuses_icc_and_rotated_or_misordered_exif_before_any_codec() {
    let plain = png(2, 1);
    let expected = TextureSource::new(plain.clone(), MapRole::BaseColor).unwrap().decode(|| false).unwrap();
    let normalized = insert_metadata(plain.clone(), b"eXIf", &normalized_exif(8, 1), true);
    let decoded = TextureSource::new(normalized.clone(), MapRole::BaseColor).unwrap().decode(|| false).unwrap();
    assert_eq!(decoded.rgba(), expected.rgba());
    assert_eq!(normalized.as_slice(), TextureSource::new(normalized.clone(), MapRole::BaseColor).unwrap().bytes.as_ref().as_slice());
    for (kind, data, before, reason) in [
        (*b"eXIf", normalized_exif(1, 6), true, "orientation"),
        (*b"eXIf", normalized_exif(1, 6), false, "precede IDAT"),
        (*b"eXIf", normalized_exif(1, 1), false, "precede IDAT"),
        (*b"iCCP", b"profile\0\0broken".to_vec(), true, "ICC"),
        (*b"iCCP", b"profile\0\0broken".to_vec(), false, "ICC"),
    ] {
        let bytes = insert_metadata(plain.clone(), &kind, &data, before);
        assert!(png_metadata::validate(&bytes).unwrap_err().to_string().contains(reason));
        assert!(TextureSource::new(bytes, MapRole::BaseColor).err().unwrap().to_string().contains(reason));
    }
}
#[test]
fn png_metadata_policy_rejects_a_small_icc_container_before_its_large_partial_inflate() {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    for _ in 0..10_240 {
        encoder.write_all(&[0; 4096]).unwrap();
    }
    let mut compressed = encoder.finish().unwrap();
    compressed.truncate(compressed.len() - 6);
    let mut icc = b"profile\0\0".to_vec();
    icc.extend(compressed);
    assert!(icc.len() < 128 * 1024, "the hostile source remains far below the existing16MiB cap");
    let bytes = insert_metadata(png(1, 1), b"iCCP", &icc, true);
    // The pure policy never constructs a decoder; missing/truncated compressed
    // contents cannot turn unsupported ICC into a None profile after inflation.
    assert!(png_metadata::validate(&bytes).unwrap_err().to_string().contains("ICC"));
    assert!(TextureSource::new(bytes, MapRole::Roughness).err().unwrap().to_string().contains("ICC"));
}
#[test]
fn png_metadata_cooperative_checks_cover_crc_ifd_tail_order_and_static_policy() {
    let source = insert_metadata(png(2, 1), b"eXIf", &normalized_exif(32, 1), true);
    let mut scanner = png_metadata::PngMetadata::new(true, true);
    let mut entries = 0;
    let mut done = false;
    for _ in 0..1000 {
        if scanner.phase_name() == "metadata_ifd" {
            entries += 1;
        }
        if scanner.step(&source).unwrap() {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(entries, 33, "32 exact IFD entries plus their bounded final transition");
    let mut corrupt = source.clone();
    let idat = corrupt.windows(4).position(|kind| kind == b"IDAT").unwrap();
    corrupt[idat + 4] ^= 0xff;
    let mut scanner = png_metadata::PngMetadata::new(true, true);
    assert!((0..1000).find_map(|_| scanner.step(&corrupt).err()).unwrap().to_string().contains("CRC"));
    for kind in [*b"IHDR", *b"PLTE", *b"ABCD", *b"acTL", *b"fcTL", *b"fdAT"] {
        let hostile = insert_metadata(png(1, 1), &kind, &[0; 13], false);
        let mut scanner = png_metadata::PngMetadata::new(true, true);
        assert!((0..1000).find_map(|_| scanner.step(&hostile).err()).is_some(), "tail {kind:?} cannot escape validation without Reader::finish");
    }
    let separated = insert_metadata(png(1, 1), b"vpAg", &[1], false);
    let separated = insert_metadata(separated, b"IDAT", &[0], false);
    let mut scanner = png_metadata::PngMetadata::new(true, true);
    assert!((0..1000).find_map(|_| scanner.step(&separated).err()).unwrap().to_string().contains("consecutive"));
}

#[test]
fn png_text_metadata_preflight_bills_real_vector_growth_under_the_existing_codec_cap() {
    let mut image = png(1, 1);
    let marker = image.windows(4).position(|kind| kind == b"IDAT").unwrap() - 4;
    let tail = image.split_off(marker);
    let entry = metadata_chunk(b"tEXt", b"k\0");
    let record_size = std::mem::size_of::<png::text_metadata::TEXtChunk>() as u64;
    let count = (MAX_CODEC_BYTES / (4 * record_size)).next_power_of_two() as usize;
    let mut within = Vec::with_capacity(image.len() + count * entry.len() + tail.len());
    within.extend(&image);
    for _ in 0..count {
        within.extend(&entry);
    }
    within.extend(&tail);
    assert!(within.len() < MAX_TEXTURE_BYTES);
    // This is a pure allocation-free policy check, not a large codec allocation.
    png_metadata::validate(&within).unwrap();
    let mut excessive = Vec::with_capacity(image.len() + (count + 1) * entry.len() + tail.len());
    excessive.extend(&image);
    for _ in 0..count + 1 {
        excessive.extend(&entry);
    }
    excessive.extend(&tail);
    assert!(excessive.len() < MAX_TEXTURE_BYTES);
    assert!(
        png_metadata::validate(&excessive).unwrap_err().to_string().contains("codec allowance"),
        "the next actual Vec<TEXtChunk> growth would overlap old/new arrays above64MiB"
    );
    assert!(TextureSource::new(excessive, MapRole::BaseColor).err().unwrap().to_string().contains("codec allowance"));
}

#[test]
#[cfg(target_pointer_width = "64")]
fn png_text_metadata_bills_latin1_old_and_new_string_backing_before_a_codec_can_run() {
    const MIB: usize = 1024 * 1024;
    let base = png(1, 1);
    let marker = base.windows(4).position(|kind| kind == b"IDAT").unwrap() - 4;
    let prefix = &base[..marker];
    let tail = &base[marker..];
    let mut latin = Vec::with_capacity(12 * MIB + 2);
    latin.extend(b"k\0");
    latin.resize(12 * MIB + 2, 0xff);
    let large = metadata_chunk(b"tEXt", &latin);
    let mut alone = Vec::with_capacity(prefix.len() + large.len() + tail.len());
    alone.extend(prefix);
    alone.extend(&large);
    alone.extend(tail);
    png_metadata::validate(&alone).unwrap(); // No arbitrary12MiB text-size prohibition.
    let count = 262_143;
    let tiny = metadata_chunk(b"tEXt", b"k\0");
    let mut hostile = Vec::with_capacity(prefix.len() + count * tiny.len() + large.len() + tail.len());
    hostile.extend(prefix);
    for _ in 0..count {
        hostile.extend(&tiny);
    }
    hostile.extend(&large);
    hostile.extend(tail);
    assert!(hostile.len() < MAX_TEXTURE_BYTES && hostile.capacity() <= MAX_TEXTURE_BYTES);
    assert_eq!(std::mem::size_of::<png::text_metadata::TEXtChunk>(), 48);
    // Independent lower bound, deliberately ignoring all small-string, fixed
    // decoder and parser storage: raw16MiB + existing text-vector12MiB +
    // old12/new24MiB Latin1 expansion already reaches the whole64MiB cap.
    // The positive keyword/fixed allocations make that an actual excess.
    let lower_bound = 16 * MIB + (count + 1) * 48 + 12 * MIB + 24 * MIB;
    assert_eq!(lower_bound as u64, MAX_CODEC_BYTES);
    assert!(png_metadata::validate(&hostile).unwrap_err().to_string().contains("codec allowance"));
    assert!(TextureSource::new(hostile, MapRole::BaseColor).err().unwrap().to_string().contains("codec allowance"));
}

fn declaration_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut result = (data.len() as u32).to_be_bytes().to_vec();
    result.extend(kind);
    result.extend(data);
    let mut crc = u32::MAX;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32.wrapping_mul(crc & 1));
        }
    }
    result.extend((!crc).to_be_bytes());
    result
}
fn declared_image(declarations: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut bytes = png(2, 2);
    let chunks: Vec<_> = declarations.iter().flat_map(|(kind, data)| declaration_chunk(kind, data)).collect();
    bytes.splice(33..33, chunks);
    bytes
}
#[test]
fn legacy_gamma_admission_preserves_encoded_pixels_and_gpu_map_interpretation() {
    let bytes = declared_image(&[(b"gAMA", 100_000_u32.to_be_bytes().to_vec())]);
    let legacy = TextureSource::new(bytes.clone(), MapRole::BaseColor).unwrap();
    assert_eq!(legacy.decode(|| false).unwrap().rgba(), [64_u8, 128, 192, 255].repeat(4));
    let map = crate::job::textures::TextureMap::new(bytes.clone(), MapRole::BaseColor).unwrap();
    let plain = crate::job::textures::TextureMap::new(png(2, 2), MapRole::BaseColor).unwrap();
    let cancel = crate::job::CancelToken::default();
    assert_eq!(map.decode(&cancel).unwrap(), plain.decode(&cancel).unwrap());
    assert!(TextureSource::new_portable(bytes, MapRole::BaseColor).err().unwrap().to_string().contains("normalized sRGB"));
}
#[test]
fn normalized_srgb_declarations_are_accepted_without_reencoding_samples() {
    let chromaticities = [31_270_u32, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000].into_iter().flat_map(u32::to_be_bytes).collect();
    let declarations = [(b"gAMA", 45_455_u32.to_be_bytes().to_vec()), (b"cHRM", chromaticities), (b"sRGB", vec![0]), (b"cICP", vec![1, 13, 0, 1])];
    for declaration in &declarations {
        let bytes = declared_image(std::slice::from_ref(declaration));
        let source = TextureSource::new_portable(bytes, MapRole::BaseColor).unwrap();
        source.validate_static_source().unwrap();
        assert!(source.decode(|| false).unwrap().rgba().as_chunks::<4>().0.iter().all(|pixel| *pixel == [64, 128, 192, 255]));
    }
    let source = TextureSource::new_portable(declared_image(&declarations), MapRole::BaseColor).unwrap();
    assert_eq!(source.decode(|| false).unwrap().rgba().len(), 16);
}
#[test]
fn conflicting_png_color_declarations_refuse_color_but_preserve_explicit_linear_data() {
    let chromaticities = [31_270_u32, 32_900, 64_000, 33_000, 30_000, 50_000, 15_000, 6_000].into_iter().flat_map(u32::to_be_bytes).collect();
    for declaration in [(b"gAMA", 100_000_u32.to_be_bytes().to_vec()), (b"cHRM", chromaticities), (b"cICP", vec![1, 16, 0, 1])] {
        let bytes = declared_image(std::slice::from_ref(&declaration));
        assert!(TextureSource::new_portable(bytes.clone(), MapRole::BaseColor).err().unwrap().to_string().contains("normalized sRGB"));
        for role in [MapRole::NormalOpenGl, MapRole::Roughness, MapRole::Metallic, MapRole::AmbientOcclusion] {
            let source = TextureSource::new_portable(bytes.clone(), role).unwrap();
            assert_eq!(source.decode(|| false).unwrap().rgba().get(..4), Some([64, 128, 192, 255].as_slice()));
        }
    }
    let duplicate = declared_image(&[(b"gAMA", 45_455_u32.to_be_bytes().to_vec()), (b"gAMA", 45_455_u32.to_be_bytes().to_vec())]);
    assert!(TextureSource::new_portable(duplicate.clone(), MapRole::BaseColor).is_err());
    let mut metadata = png_metadata::PngMetadata::new(true, true).with_role(MapRole::BaseColor);
    let error = (0..100).find_map(|_| metadata.step(&duplicate).err()).unwrap();
    assert!(error.to_string().contains("normalized sRGB"), "cooperative color checks retain duplicate-declaration state between steps");
    for kind in [b"mDCV", b"cLLI"] {
        assert!(validate_color_declarations(&declared_image(&[(kind, vec![0; 24])]), MapRole::BaseColor).is_err());
    }
}
