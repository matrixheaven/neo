use std::{borrow::Cow, error::Error, fmt, io::Cursor};

use serde::{Deserialize, Serialize};

pub mod iterm2;
pub mod kitty;
pub mod sixel;

pub use iterm2::{Iterm2Dimension, Iterm2InlineImageOptions, encode_iterm2_inline_image};
pub use kitty::{KittyGraphicsOptions, KittyImageFormat, encode_kitty_graphics};
pub use sixel::{SixelImageOptions, SixelPaletteColor, encode_sixel_image};

pub(super) const STRING_TERMINATOR: &str = "\x1b\\";
const MAX_KITTY_IMAGE_DIMENSION: u32 = 16_384;
const MAX_KITTY_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_KITTY_IMAGE_ALLOC_BYTES: u64 = MAX_KITTY_IMAGE_PIXELS * 4;
const MAX_KITTY_IMAGE_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageProtocolPreference {
    #[default]
    Auto,
    Kitty,
    Iterm2,
    Sixel,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NegotiatedImageProtocol {
    #[default]
    None,
    Kitty,
    Iterm2,
    Sixel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalImageCapabilities {
    kitty: bool,
    iterm2: bool,
    sixel: bool,
}

impl TerminalImageCapabilities {
    #[must_use]
    pub const fn kitty(self) -> bool {
        self.kitty
    }

    #[must_use]
    pub const fn iterm2(self) -> bool {
        self.iterm2
    }

    #[must_use]
    pub const fn sixel(self) -> bool {
        self.sixel
    }

    #[must_use]
    pub const fn with_kitty(mut self, supported: bool) -> Self {
        self.kitty = supported;
        self
    }

    #[must_use]
    pub const fn with_iterm2(mut self, supported: bool) -> Self {
        self.iterm2 = supported;
        self
    }

    #[must_use]
    pub const fn with_sixel(mut self, supported: bool) -> Self {
        self.sixel = supported;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRenderPolicy {
    protocol: ImageProtocolPreference,
}

impl ImageRenderPolicy {
    #[must_use]
    pub const fn new(protocol: ImageProtocolPreference) -> Self {
        Self { protocol }
    }

    #[must_use]
    pub const fn protocol(self) -> ImageProtocolPreference {
        self.protocol
    }

    #[must_use]
    pub fn negotiate(self, capabilities: TerminalImageCapabilities) -> NegotiatedImageProtocol {
        match self.protocol {
            ImageProtocolPreference::Auto => {
                if capabilities.kitty {
                    NegotiatedImageProtocol::Kitty
                } else if capabilities.iterm2 {
                    NegotiatedImageProtocol::Iterm2
                } else {
                    NegotiatedImageProtocol::None
                }
            }
            ImageProtocolPreference::Kitty if capabilities.kitty => NegotiatedImageProtocol::Kitty,
            ImageProtocolPreference::Iterm2 if capabilities.iterm2 => {
                NegotiatedImageProtocol::Iterm2
            }
            ImageProtocolPreference::Sixel if capabilities.sixel => NegotiatedImageProtocol::Sixel,
            ImageProtocolPreference::Kitty
            | ImageProtocolPreference::Iterm2
            | ImageProtocolPreference::Sixel
            | ImageProtocolPreference::None => NegotiatedImageProtocol::None,
        }
    }

    #[must_use]
    pub fn render_inline_image(
        self,
        image: &InlineImage,
        capabilities: TerminalImageCapabilities,
        display: &ImageDisplayOptions,
    ) -> RenderedInlineImage {
        let metadata = image.metadata_summary();
        let fallback = display.fallback_line(metadata.clone());
        let Some(bytes) = image.data_bytes() else {
            return RenderedInlineImage {
                metadata,
                protocol: NegotiatedImageProtocol::None,
                lines: vec![fallback],
                escape_sequence: None,
            };
        };
        self.render_inline_image_bytes(
            &image.id,
            &image.mime_type,
            bytes,
            metadata,
            capabilities,
            display,
        )
    }

    pub(crate) fn render_inline_image_bytes(
        self,
        id: &str,
        mime_type: &str,
        bytes: &[u8],
        metadata: String,
        capabilities: TerminalImageCapabilities,
        display: &ImageDisplayOptions,
    ) -> RenderedInlineImage {
        let fallback = display.fallback_line(metadata.clone());
        let Some((cell_width, cell_height)) = display.cell_size() else {
            return RenderedInlineImage {
                metadata,
                protocol: NegotiatedImageProtocol::None,
                lines: vec![fallback],
                escape_sequence: None,
            };
        };

        let protocol = self.negotiate(capabilities);
        let escape_sequence = match protocol {
            NegotiatedImageProtocol::Kitty => {
                normalize_kitty_payload(bytes, mime_type).and_then(|png| {
                    encode_kitty_graphics(
                        png.as_ref(),
                        &KittyGraphicsOptions::new(KittyImageFormat::Png)
                            .with_image_id(stable_image_id(id))
                            .with_cell_size(cell_width, cell_height),
                    )
                    .ok()
                })
            }
            NegotiatedImageProtocol::Iterm2 => encode_iterm2_inline_image(
                bytes,
                &Iterm2InlineImageOptions::new()
                    .with_name(id)
                    .with_width(Iterm2Dimension::Cells(cell_width))
                    .with_height(Iterm2Dimension::Cells(cell_height)),
            )
            .ok(),
            NegotiatedImageProtocol::Sixel | NegotiatedImageProtocol::None => None,
        };
        let lines = escape_sequence.as_ref().map_or_else(
            || vec![fallback],
            |sequence| image_lines(sequence, protocol, cell_height),
        );

        RenderedInlineImage {
            metadata,
            protocol: if escape_sequence.is_some() {
                protocol
            } else {
                NegotiatedImageProtocol::None
            },
            lines,
            escape_sequence,
        }
    }
}

impl Default for ImageRenderPolicy {
    fn default() -> Self {
        Self::new(ImageProtocolPreference::Auto)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedInlineImage {
    pub metadata: String,
    pub protocol: NegotiatedImageProtocol,
    pub lines: Vec<String>,
    pub escape_sequence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDisplayOptions {
    source_width: u32,
    source_height: u32,
    max_cols: u32,
    max_rows: u32,
    placeholder: Option<String>,
}

impl ImageDisplayOptions {
    pub const DEFAULT_MAX_COLS: u32 = 40;
    pub const DEFAULT_MAX_ROWS: u32 = 12;

    #[must_use]
    pub fn thumbnail(
        source_width: u32,
        source_height: u32,
        placeholder: impl Into<String>,
    ) -> Self {
        Self {
            source_width,
            source_height,
            max_cols: Self::DEFAULT_MAX_COLS,
            max_rows: Self::DEFAULT_MAX_ROWS,
            placeholder: Some(placeholder.into()),
        }
    }

    #[must_use]
    pub const fn bounded(source_width: u32, source_height: u32) -> Self {
        Self {
            source_width,
            source_height,
            max_cols: Self::DEFAULT_MAX_COLS,
            max_rows: Self::DEFAULT_MAX_ROWS,
            placeholder: None,
        }
    }

    #[must_use]
    pub const fn with_max_cols(mut self, max_cols: u32) -> Self {
        self.max_cols = max_cols;
        self
    }

    #[must_use]
    pub const fn with_max_rows(mut self, max_rows: u32) -> Self {
        self.max_rows = max_rows;
        self
    }

    fn fallback_line(&self, metadata: String) -> String {
        self.placeholder.clone().unwrap_or(metadata)
    }

    fn cell_size(&self) -> Option<(u32, u32)> {
        if self.source_width == 0
            || self.source_height == 0
            || self.max_cols == 0
            || self.max_rows == 0
        {
            return None;
        }
        let source_width = u64::from(self.source_width);
        let source_height = u64::from(self.source_height);
        let max_cols = u64::from(self.max_cols);
        let max_rows = u64::from(self.max_rows);

        let width_limited_rows = div_round(max_cols * source_height, source_width).max(1);
        if width_limited_rows <= max_rows {
            return Some((
                self.max_cols,
                u32::try_from(width_limited_rows).unwrap_or(self.max_rows),
            ));
        }
        let height_limited_cols = div_round(max_rows * source_width, source_height).max(1);
        Some((
            u32::try_from(height_limited_cols.min(max_cols)).unwrap_or(self.max_cols),
            self.max_rows,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource {
    Local,
    Base64,
    Generated,
    RemoteUrl,
}

impl ImageSource {
    const fn metadata_label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Base64 => "data",
            Self::Generated => "generated",
            Self::RemoteUrl => "url",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImage {
    pub id: String,
    pub mime_type: String,
    pub alt: Option<String>,
    pub source: ImageSource,
    payload: InlineImagePayload,
}

impl InlineImage {
    #[must_use]
    pub fn bytes(
        id: impl Into<String>,
        mime_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        alt: Option<impl Into<String>>,
        source: ImageSource,
    ) -> Self {
        Self {
            id: id.into(),
            mime_type: mime_type.into(),
            alt: alt.map(Into::into),
            source,
            payload: InlineImagePayload::Bytes(bytes.into()),
        }
    }

    #[must_use]
    pub fn remote_url(
        id: impl Into<String>,
        mime_type: impl Into<String>,
        url: impl Into<String>,
        alt: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            mime_type: mime_type.into(),
            alt: alt.map(Into::into),
            source: ImageSource::RemoteUrl,
            payload: InlineImagePayload::RemoteUrl(url.into()),
        }
    }

    #[must_use]
    pub fn size_bytes(&self) -> Option<usize> {
        self.data_bytes().map(<[u8]>::len)
    }

    #[must_use]
    pub fn metadata_summary(&self) -> String {
        let mut summary = String::from("[image: ");
        summary.push_str(&self.mime_type);
        summary.push(' ');
        match &self.payload {
            InlineImagePayload::Bytes(bytes) => match self.source {
                ImageSource::Base64 => {
                    summary.push_str("data=");
                    summary.push_str(&bytes.len().to_string());
                    summary.push_str(" bytes");
                }
                source => {
                    summary.push_str(source.metadata_label());
                    summary.push(' ');
                    summary.push_str(&bytes.len().to_string());
                    summary.push_str(" bytes");
                }
            },
            InlineImagePayload::RemoteUrl(url) => {
                summary.push_str("url=");
                summary.push_str(url);
            }
        }
        if let Some(alt) = &self.alt {
            summary.push_str(" alt=\"");
            summary.push_str(&escape_metadata_value(alt));
            summary.push('"');
        }
        summary.push(']');
        summary
    }

    #[must_use]
    pub fn into_payload_bytes(self) -> Option<Vec<u8>> {
        match self.payload {
            InlineImagePayload::Bytes(bytes) => Some(bytes),
            InlineImagePayload::RemoteUrl(_) => None,
        }
    }

    fn data_bytes(&self) -> Option<&[u8]> {
        match &self.payload {
            InlineImagePayload::Bytes(bytes) => Some(bytes),
            InlineImagePayload::RemoteUrl(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InlineImagePayload {
    Bytes(Vec<u8>),
    RemoteUrl(String),
}

fn normalize_kitty_payload<'a>(bytes: &'a [u8], mime_type: &str) -> Option<Cow<'a, [u8]>> {
    if bytes.len() > MAX_KITTY_IMAGE_PAYLOAD_BYTES {
        return None;
    }
    let input_format = match mime_type {
        "image/png" => {
            if !validate_kitty_png(bytes)? {
                return Some(Cow::Borrowed(bytes));
            }
            image::ImageFormat::Png
        }
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/gif" => image::ImageFormat::Gif,
        "image/webp" => image::ImageFormat::WebP,
        _ => return None,
    };
    safe_kitty_dimensions(detect_image_dimensions(bytes, mime_type)?)?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), input_format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_KITTY_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_KITTY_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_KITTY_IMAGE_ALLOC_BYTES);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(Cow::Owned(png.into_inner()))
}

fn validate_kitty_png(bytes: &[u8]) -> Option<bool> {
    let mut cursor = Cursor::new(bytes);
    let decoder = png::Decoder::new_with_limits(
        &mut cursor,
        png::Limits {
            bytes: MAX_KITTY_IMAGE_PAYLOAD_BYTES,
        },
    );
    let mut reader = decoder.read_info().ok()?;
    safe_kitty_dimensions((reader.info().width, reader.info().height))?;
    let is_animated = reader.info().animation_control.is_some();
    while reader.next_row().ok()?.is_some() {}
    reader.finish().ok()?;
    drop(reader);
    (usize::try_from(cursor.position()).ok()? == bytes.len()).then_some(is_animated)
}

fn safe_kitty_dimensions((width, height): (u32, u32)) -> Option<()> {
    if width == 0
        || height == 0
        || width > MAX_KITTY_IMAGE_DIMENSION
        || height > MAX_KITTY_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_KITTY_IMAGE_PIXELS
    {
        return None;
    }
    Some(())
}

#[must_use]
pub fn detect_image_dimensions(bytes: &[u8], mime_type: &str) -> Option<(u32, u32)> {
    match mime_type {
        "image/png" => png_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24
        || &bytes[0..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[8..12] != 13_u32.to_be_bytes().as_slice()
        || &bytes[12..16] != b"IHDR"
    {
        return None;
    }
    Some((
        u32::from_be_bytes(bytes[16..20].try_into().ok()?),
        u32::from_be_bytes(bytes[20..24].try_into().ok()?),
    ))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 0;
    while index + 8 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            index += 2;
            continue;
        }
        if index + 4 >= bytes.len() {
            break;
        }
        let segment_len = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if marker == 0xC0 || marker == 0xC1 || marker == 0xC2 {
            if index + 9 >= bytes.len() {
                break;
            }
            let height = u32::from(u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]));
            let width = u32::from(u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]));
            return Some((width, height));
        }
        index += 2 + segment_len;
    }
    None
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 || &bytes[0..3] != b"GIF" {
        return None;
    }
    Some((
        u32::from(u16::from_le_bytes([bytes[6], bytes[7]])),
        u32::from(u16::from_le_bytes([bytes[8], bytes[9]])),
    ))
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    if &bytes[12..16] == b"VP8 " {
        return Some((
            u32::from(u16::from_le_bytes([bytes[26], bytes[27]])) & 0x3FFF,
            u32::from(u16::from_le_bytes([bytes[28], bytes[29]])) & 0x3FFF,
        ));
    }
    if &bytes[12..16] == b"VP8L" {
        if bytes.len() < 25 {
            return None;
        }
        let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
        return Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1));
    }
    None
}

fn stable_image_id(id: &str) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for byte in id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash.max(1)
}

fn escape_metadata_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn image_lines(
    escape_sequence: &str,
    protocol: NegotiatedImageProtocol,
    cell_height: u32,
) -> Vec<String> {
    let reserved_rows = if matches!(protocol, NegotiatedImageProtocol::Kitty) {
        cell_height.max(1) as usize
    } else {
        1
    };
    let mut lines = Vec::with_capacity(reserved_rows);
    lines.push(escape_sequence.to_owned());
    lines.extend(std::iter::repeat_n(
        String::new(),
        reserved_rows.saturating_sub(1),
    ));
    lines
}

fn div_round(numerator: u64, denominator: u64) -> u64 {
    (numerator + denominator / 2) / denominator
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocolError {
    EmptyImageData,
    InvalidChunkSize,
    InvalidColorIndex,
    InvalidDimension,
    InvalidPalette,
    InvalidPixelDataLength,
}

impl ImageProtocolError {
    const fn message(self) -> &'static str {
        match self {
            Self::EmptyImageData => "image data must not be empty",
            Self::InvalidChunkSize => "kitty chunk size must be greater than zero",
            Self::InvalidColorIndex => "sixel pixel data contains a palette index out of range",
            Self::InvalidDimension => "image dimensions must be greater than zero",
            Self::InvalidPalette => {
                "sixel palette must not be empty and RGB percentage values must be <= 100"
            }
            Self::InvalidPixelDataLength => {
                "sixel pixel data length must exactly match image width multiplied by height"
            }
        }
    }
}

impl fmt::Display for ImageProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl Error for ImageProtocolError {}

pub(super) fn validate_u32_dimension(value: u32) -> Result<(), ImageProtocolError> {
    if value == 0 {
        Err(ImageProtocolError::InvalidDimension)
    } else {
        Ok(())
    }
}

pub(super) fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use image::{ColorType, ImageEncoder as _, codecs::jpeg::JpegEncoder};

    use super::*;

    fn test_png() -> Vec<u8> {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[0, 0, 0], 1, 1, ColorType::Rgb8.into())
            .expect("encode test PNG");
        png
    }

    fn test_apng() -> Vec<u8> {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 0).expect("configure APNG");
        let mut writer = encoder.write_header().expect("write APNG header");
        writer
            .write_image_data(&[255, 0, 0])
            .expect("write first APNG frame");
        writer
            .write_image_data(&[0, 255, 0])
            .expect("write second APNG frame");
        writer.finish().expect("finish APNG");
        png
    }

    #[test]
    fn kitty_png_payload_preserves_original_bytes() {
        let png = test_png();

        let normalized = normalize_kitty_payload(&png, "image/png").expect("valid PNG");

        assert!(matches!(normalized, Cow::Borrowed(_)));
        assert_eq!(normalized.as_ref(), png);
    }

    #[test]
    fn kitty_apng_payload_becomes_static_owned_png() {
        let apng = test_apng();

        let normalized = normalize_kitty_payload(&apng, "image/png").expect("valid APNG");

        assert!(matches!(normalized, Cow::Owned(_)));
        assert!(!normalized.windows(4).any(|window| window == b"acTL"));
        assert!(
            image::load_from_memory_with_format(normalized.as_ref(), image::ImageFormat::Png)
                .is_ok()
        );
    }

    #[test]
    fn kitty_png_payload_rejects_bad_crc() {
        let mut png = test_png();
        *png.last_mut().expect("IEND CRC") ^= 1;

        assert!(normalize_kitty_payload(&png, "image/png").is_none());
    }

    #[test]
    fn kitty_png_payload_rejects_bad_zlib_with_valid_chunk_crc() {
        let mut png = test_png();
        let chunk_type = png
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("IDAT chunk");
        let data_len = u32::from_be_bytes(
            png[chunk_type - 4..chunk_type]
                .try_into()
                .expect("IDAT length"),
        ) as usize;
        let data_start = chunk_type + 4;
        let data_end = chunk_type + 4 + data_len;
        png[data_start] = 0;
        let crc = crc32fast::hash(&png[chunk_type..data_end]);
        png[data_end..data_end + 4].copy_from_slice(&crc.to_be_bytes());

        assert!(normalize_kitty_payload(&png, "image/png").is_none());
    }

    #[test]
    fn kitty_png_payload_rejects_truncation() {
        let mut png = test_png();
        png.pop();

        assert!(normalize_kitty_payload(&png, "image/png").is_none());
    }

    #[test]
    fn kitty_png_payload_rejects_excessive_encoded_bytes() {
        let mut png = test_png();
        png.resize(MAX_KITTY_IMAGE_PAYLOAD_BYTES + 1, 0);

        assert!(normalize_kitty_payload(&png, "image/png").is_none());
    }

    #[test]
    fn kitty_png_payload_rejects_excessive_pixel_count() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend_from_slice(&10_000_u32.to_be_bytes());
        header.extend_from_slice(&5_000_u32.to_be_bytes());

        assert!(normalize_kitty_payload(&header, "image/png").is_none());
    }

    #[test]
    fn kitty_jpeg_payload_is_png_or_falls_back() {
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg)
            .encode(&[0, 0, 0], 1, 1, ColorType::Rgb8.into())
            .expect("encode test JPEG");
        let image = InlineImage::bytes(
            "jpeg",
            "image/jpeg",
            jpeg,
            None::<String>,
            ImageSource::Generated,
        );
        let rendered = ImageRenderPolicy::new(ImageProtocolPreference::Kitty).render_inline_image(
            &image,
            TerminalImageCapabilities::default().with_kitty(true),
            &ImageDisplayOptions::bounded(1, 1),
        );

        assert_eq!(rendered.protocol, NegotiatedImageProtocol::Kitty);
        let sequence = rendered.escape_sequence.expect("Kitty sequence");
        let encoded = sequence
            .split_once(';')
            .expect("Kitty payload separator")
            .1
            .strip_suffix(STRING_TERMINATOR)
            .expect("Kitty string terminator");
        let payload = STANDARD.decode(encoded).expect("Kitty payload base64");
        assert!(payload.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(sequence.contains("f=100"));
    }

    #[test]
    fn kitty_invalid_payload_falls_back_to_metadata() {
        for (mime_type, bytes) in [
            ("image/jpeg", b"not a JPEG".as_slice()),
            ("image/png", b"\x89PNG".as_slice()),
        ] {
            let image = InlineImage::bytes(
                "invalid",
                mime_type,
                bytes.to_vec(),
                None::<String>,
                ImageSource::Generated,
            );
            let rendered = ImageRenderPolicy::new(ImageProtocolPreference::Kitty)
                .render_inline_image(
                    &image,
                    TerminalImageCapabilities::default().with_kitty(true),
                    &ImageDisplayOptions::bounded(1, 1),
                );

            assert_eq!(rendered.protocol, NegotiatedImageProtocol::None);
            assert!(rendered.escape_sequence.is_none());
            assert_eq!(rendered.lines, vec![rendered.metadata]);
        }
    }
}
