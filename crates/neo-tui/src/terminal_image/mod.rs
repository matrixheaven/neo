use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub mod iterm2;
pub mod kitty;
pub mod sixel;

pub use iterm2::{Iterm2Dimension, Iterm2InlineImageOptions, encode_iterm2_inline_image};
pub use kitty::{KittyGraphicsOptions, KittyImageFormat, encode_kitty_graphics};
pub use sixel::{SixelImageOptions, SixelPaletteColor, encode_sixel_image};

pub(super) const STRING_TERMINATOR: &str = "\x1b\\";

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
    fetch_remote_images: bool,
}

impl ImageRenderPolicy {
    #[must_use]
    pub const fn new(protocol: ImageProtocolPreference, fetch_remote_images: bool) -> Self {
        Self {
            protocol,
            fetch_remote_images,
        }
    }

    #[must_use]
    pub const fn protocol(self) -> ImageProtocolPreference {
        self.protocol
    }

    #[must_use]
    pub const fn fetch_remote_images(self) -> bool {
        self.fetch_remote_images
    }

    #[must_use]
    pub fn negotiate(self, capabilities: TerminalImageCapabilities) -> NegotiatedImageProtocol {
        match self.protocol {
            ImageProtocolPreference::Auto => {
                if capabilities.kitty {
                    NegotiatedImageProtocol::Kitty
                } else if capabilities.iterm2 {
                    NegotiatedImageProtocol::Iterm2
                } else if capabilities.sixel {
                    NegotiatedImageProtocol::Sixel
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
    ) -> RenderedInlineImage {
        let metadata = image.metadata_summary();
        let Some(bytes) = image.data_bytes() else {
            return RenderedInlineImage {
                metadata,
                protocol: NegotiatedImageProtocol::None,
                escape_sequence: None,
            };
        };
        if image.is_remote() && !self.fetch_remote_images {
            return RenderedInlineImage {
                metadata,
                protocol: NegotiatedImageProtocol::None,
                escape_sequence: None,
            };
        }

        let protocol = self.negotiate(capabilities);
        let escape_sequence = match protocol {
            NegotiatedImageProtocol::Kitty => encode_kitty_graphics(
                bytes,
                &KittyGraphicsOptions::new(kitty_format_for_mime(&image.mime_type))
                    .with_image_id(stable_image_id(&image.id)),
            )
            .ok(),
            NegotiatedImageProtocol::Iterm2 => encode_iterm2_inline_image(
                bytes,
                &Iterm2InlineImageOptions::new().with_name(image.id.clone()),
            )
            .ok(),
            NegotiatedImageProtocol::Sixel => render_bytes_as_sixel(bytes).ok(),
            NegotiatedImageProtocol::None => None,
        };

        RenderedInlineImage {
            metadata,
            protocol: if escape_sequence.is_some() {
                protocol
            } else {
                NegotiatedImageProtocol::None
            },
            escape_sequence,
        }
    }
}

impl Default for ImageRenderPolicy {
    fn default() -> Self {
        Self::new(ImageProtocolPreference::Auto, false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedInlineImage {
    pub metadata: String,
    pub protocol: NegotiatedImageProtocol,
    pub escape_sequence: Option<String>,
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

    fn is_remote(&self) -> bool {
        matches!(&self.payload, InlineImagePayload::RemoteUrl(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InlineImagePayload {
    Bytes(Vec<u8>),
    RemoteUrl(String),
}

fn kitty_format_for_mime(_mime_type: &str) -> KittyImageFormat {
    KittyImageFormat::Png
}

fn stable_image_id(id: &str) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for byte in id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash.max(1)
}

fn render_bytes_as_sixel(bytes: &[u8]) -> Result<String, ImageProtocolError> {
    let color = if bytes.is_empty() { 0 } else { bytes[0] % 2 };
    encode_sixel_image(
        &[color],
        &SixelImageOptions::new(
            1,
            1,
            vec![
                SixelPaletteColor::rgb_percent(0, 0, 0),
                SixelPaletteColor::rgb_percent(100, 100, 100),
            ],
        ),
    )
}

fn escape_metadata_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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
