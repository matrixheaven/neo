use super::{ImageProtocolError, encode_base64, validate_u32_dimension};

const OSC_1337_FILE_START: &str = "\x1b]1337;File=";
const OSC_TERMINATOR: char = '\x07';

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
