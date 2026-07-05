use super::{ImageProtocolError, STRING_TERMINATOR, encode_base64, validate_u32_dimension};

const KITTY_START: &str = "\x1b_G";

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
    cell_width: Option<u32>,
    cell_height: Option<u32>,
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
            cell_width: None,
            cell_height: None,
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
    pub const fn with_cell_size(mut self, width: u32, height: u32) -> Self {
        self.cell_width = Some(width);
        self.cell_height = Some(height);
        self
    }

    #[must_use]
    pub const fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
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
    validate_optional_dimension(options.cell_width)?;
    validate_optional_dimension(options.cell_height)?;

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
    if let Some(width) = options.cell_width {
        parameters.push(("c", width.to_string()));
    }
    if let Some(height) = options.cell_height {
        parameters.push(("r", height.to_string()));
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
