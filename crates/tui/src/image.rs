use std::{error::Error, fmt};

const KITTY_START: &str = "\x1b_G";
const STRING_TERMINATOR: &str = "\x1b\\";
const OSC_1337_FILE_START: &str = "\x1b]1337;File=";
const OSC_TERMINATOR: char = '\x07';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocolError {
    EmptyImageData,
    InvalidChunkSize,
    InvalidDimension,
}

impl fmt::Display for ImageProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyImageData => formatter.write_str("image data must not be empty"),
            Self::InvalidChunkSize => {
                formatter.write_str("kitty chunk size must be greater than zero")
            }
            Self::InvalidDimension => {
                formatter.write_str("image dimensions must be greater than zero")
            }
        }
    }
}

impl Error for ImageProtocolError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyImageFormat {
    Png,
    Rgb,
    Rgba,
}

impl KittyImageFormat {
    const fn protocol_value(self) -> u16 {
        match self {
            Self::Png => 100,
            Self::Rgb => 24,
            Self::Rgba => 32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyGraphicsOptions {
    format: KittyImageFormat,
    image_id: Option<u32>,
    pixel_width: Option<u32>,
    pixel_height: Option<u32>,
    chunk_size: usize,
}

impl KittyGraphicsOptions {
    pub const DEFAULT_CHUNK_SIZE: usize = 4096;

    #[must_use]
    pub const fn new(format: KittyImageFormat) -> Self {
        Self {
            format,
            image_id: None,
            pixel_width: None,
            pixel_height: None,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
        }
    }

    #[must_use]
    pub const fn with_image_id(mut self, image_id: u32) -> Self {
        self.image_id = Some(image_id);
        self
    }

    #[must_use]
    pub const fn with_pixel_size(mut self, width: u32, height: u32) -> Self {
        self.pixel_width = Some(width);
        self.pixel_height = Some(height);
        self
    }

    #[must_use]
    pub const fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Iterm2Dimension {
    Cells(u32),
    Pixels(u32),
    Percent(u8),
    Auto,
}

impl Iterm2Dimension {
    fn write_metadata(self, field: &str, output: &mut String) -> Result<(), ImageProtocolError> {
        match self {
            Self::Cells(value) => {
                validate_u32_dimension(value)?;
                output.push(';');
                output.push_str(field);
                output.push('=');
                output.push_str(&value.to_string());
            }
            Self::Pixels(value) => {
                validate_u32_dimension(value)?;
                output.push(';');
                output.push_str(field);
                output.push('=');
                output.push_str(&value.to_string());
                output.push_str("px");
            }
            Self::Percent(value) => {
                if value == 0 {
                    return Err(ImageProtocolError::InvalidDimension);
                }
                output.push(';');
                output.push_str(field);
                output.push('=');
                output.push_str(&value.to_string());
                output.push('%');
            }
            Self::Auto => {
                output.push(';');
                output.push_str(field);
                output.push_str("=auto");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iterm2InlineImageOptions {
    name: Option<String>,
    width: Option<Iterm2Dimension>,
    height: Option<Iterm2Dimension>,
    preserve_aspect_ratio: bool,
}

impl Iterm2InlineImageOptions {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            name: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
        }
    }

    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[must_use]
    pub const fn with_width(mut self, width: Iterm2Dimension) -> Self {
        self.width = Some(width);
        self
    }

    #[must_use]
    pub const fn with_height(mut self, height: Iterm2Dimension) -> Self {
        self.height = Some(height);
        self
    }

    #[must_use]
    pub const fn with_preserve_aspect_ratio(mut self, preserve: bool) -> Self {
        self.preserve_aspect_ratio = preserve;
        self
    }
}

impl Default for Iterm2InlineImageOptions {
    fn default() -> Self {
        Self::new()
    }
}

pub fn encode_kitty_graphics(
    data: &[u8],
    options: &KittyGraphicsOptions,
) -> Result<String, ImageProtocolError> {
    if data.is_empty() {
        return Err(ImageProtocolError::EmptyImageData);
    }
    if options.chunk_size == 0 {
        return Err(ImageProtocolError::InvalidChunkSize);
    }
    validate_optional_dimension(options.pixel_width)?;
    validate_optional_dimension(options.pixel_height)?;

    let encoded = encode_base64(data);
    let mut output = String::new();
    let mut chunks = encoded.as_bytes().chunks(options.chunk_size).peekable();

    if let Some(first_chunk) = chunks.next() {
        write_kitty_sequence(
            &mut output,
            &first_kitty_parameters(options, chunks.peek().is_some()),
            first_chunk,
        );
    }

    while let Some(chunk) = chunks.next() {
        let has_more = chunks.peek().is_some();
        write_kitty_sequence(
            &mut output,
            &[("m", if has_more { "1" } else { "0" }.to_owned())],
            chunk,
        );
    }

    Ok(output)
}

pub fn encode_iterm2_inline_image(
    data: &[u8],
    options: &Iterm2InlineImageOptions,
) -> Result<String, ImageProtocolError> {
    if data.is_empty() {
        return Err(ImageProtocolError::EmptyImageData);
    }

    let mut output = String::from(OSC_1337_FILE_START);
    if let Some(name) = &options.name {
        output.push_str("name=");
        output.push_str(&encode_base64(name.as_bytes()));
    }
    if let Some(width) = options.width {
        width.write_metadata("width", &mut output)?;
    }
    if let Some(height) = options.height {
        height.write_metadata("height", &mut output)?;
    }
    if !options.preserve_aspect_ratio {
        output.push_str(";preserveAspectRatio=0");
    }
    output.push_str(";inline=1:");
    output.push_str(&encode_base64(data));
    output.push(OSC_TERMINATOR);
    Ok(output)
}

fn first_kitty_parameters(
    options: &KittyGraphicsOptions,
    has_more: bool,
) -> Vec<(&'static str, String)> {
    let mut parameters = vec![
        ("a", "T".to_owned()),
        ("f", options.format.protocol_value().to_string()),
        ("t", "d".to_owned()),
    ];
    if let Some(image_id) = options.image_id {
        parameters.push(("i", image_id.to_string()));
    }
    if let Some(width) = options.pixel_width {
        parameters.push(("s", width.to_string()));
    }
    if let Some(height) = options.pixel_height {
        parameters.push(("v", height.to_string()));
    }
    if has_more {
        parameters.push(("m", "1".to_owned()));
    }
    parameters
}

fn write_kitty_sequence(output: &mut String, parameters: &[(&str, String)], payload: &[u8]) {
    output.push_str(KITTY_START);
    for (index, (key, value)) in parameters.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(key);
        output.push('=');
        output.push_str(value);
    }
    output.push(';');
    output.push_str(std::str::from_utf8(payload).expect("base64 is valid utf-8"));
    output.push_str(STRING_TERMINATOR);
}

fn validate_optional_dimension(dimension: Option<u32>) -> Result<(), ImageProtocolError> {
    if let Some(value) = dimension {
        validate_u32_dimension(value)?;
    }
    Ok(())
}

fn validate_u32_dimension(value: u32) -> Result<(), ImageProtocolError> {
    if value == 0 {
        Err(ImageProtocolError::InvalidDimension)
    } else {
        Ok(())
    }
}

fn encode_base64(data: &[u8]) -> String {
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
