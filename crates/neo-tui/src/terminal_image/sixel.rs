use super::{ImageProtocolError, STRING_TERMINATOR, validate_u32_dimension};

const SIXEL_START: &str = "\x1bPq";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SixelPaletteColor {
    red: u8,
    green: u8,
    blue: u8,
}

impl SixelPaletteColor {
    #[must_use]
    pub const fn rgb_percent(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    const fn is_valid(self) -> bool {
        self.red <= 100 && self.green <= 100 && self.blue <= 100
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SixelImageOptions {
    pixel_width: u32,
    pixel_height: u32,
    palette: Vec<SixelPaletteColor>,
}

impl SixelImageOptions {
    #[must_use]
    pub const fn new(pixel_width: u32, pixel_height: u32, palette: Vec<SixelPaletteColor>) -> Self {
        Self {
            pixel_width,
            pixel_height,
            palette,
        }
    }
}

pub fn encode_sixel_image(
    indexed_pixels: &[u8],
    options: &SixelImageOptions,
) -> Result<String, ImageProtocolError> {
    if indexed_pixels.is_empty() {
        return Err(ImageProtocolError::EmptyImageData);
    }
    validate_u32_dimension(options.pixel_width)?;
    validate_u32_dimension(options.pixel_height)?;
    validate_sixel_palette(&options.palette)?;

    let pixel_count = (options.pixel_width as usize)
        .checked_mul(options.pixel_height as usize)
        .ok_or(ImageProtocolError::InvalidPixelDataLength)?;
    if indexed_pixels.len() != pixel_count {
        return Err(ImageProtocolError::InvalidPixelDataLength);
    }
    if indexed_pixels
        .iter()
        .any(|index| usize::from(*index) >= options.palette.len())
    {
        return Err(ImageProtocolError::InvalidColorIndex);
    }

    let mut output = String::from(SIXEL_START);
    write_sixel_raster_attributes(&mut output, options.pixel_width, options.pixel_height);
    write_sixel_palette(&mut output, &options.palette);
    write_sixel_pixels(&mut output, indexed_pixels, options);
    output.push_str(STRING_TERMINATOR);
    Ok(output)
}

fn validate_sixel_palette(palette: &[SixelPaletteColor]) -> Result<(), ImageProtocolError> {
    if palette.is_empty() || palette.iter().any(|color| !color.is_valid()) {
        Err(ImageProtocolError::InvalidPalette)
    } else {
        Ok(())
    }
}

fn write_sixel_raster_attributes(output: &mut String, width: u32, height: u32) {
    output.push('"');
    output.push_str("1;1;");
    output.push_str(&width.to_string());
    output.push(';');
    output.push_str(&height.to_string());
}

fn write_sixel_palette(output: &mut String, palette: &[SixelPaletteColor]) {
    for (index, color) in palette.iter().enumerate() {
        output.push('#');
        output.push_str(&index.to_string());
        output.push_str(";2;");
        output.push_str(&color.red.to_string());
        output.push(';');
        output.push_str(&color.green.to_string());
        output.push(';');
        output.push_str(&color.blue.to_string());
    }
}

fn write_sixel_pixels(output: &mut String, indexed_pixels: &[u8], options: &SixelImageOptions) {
    let width = options.pixel_width as usize;
    let height = options.pixel_height as usize;

    for band_start in (0..height).step_by(6) {
        if band_start > 0 {
            output.push('-');
        }
        write_sixel_band(
            output,
            indexed_pixels,
            width,
            height,
            band_start,
            &options.palette,
        );
    }
}

fn write_sixel_band(
    output: &mut String,
    indexed_pixels: &[u8],
    width: usize,
    height: usize,
    band_start: usize,
    palette: &[SixelPaletteColor],
) {
    let mut wrote_color_plane = false;
    for color_index in 0..palette.len() {
        let plane = sixel_color_plane(indexed_pixels, width, height, band_start, color_index);
        if plane.iter().all(|bits| *bits == 0) {
            continue;
        }
        if wrote_color_plane {
            output.push('$');
        }
        output.push('#');
        output.push_str(&color_index.to_string());
        for bits in plane {
            output.push(sixel_char(bits));
        }
        wrote_color_plane = true;
    }
}

fn sixel_color_plane(
    indexed_pixels: &[u8],
    width: usize,
    height: usize,
    band_start: usize,
    color_index: usize,
) -> Vec<u8> {
    let mut plane = vec![0; width];
    for (x, bits) in plane.iter_mut().enumerate() {
        for bit in 0..6 {
            let y = band_start + bit;
            if y >= height {
                break;
            }
            let pixel_index = y * width + x;
            if usize::from(indexed_pixels[pixel_index]) == color_index {
                *bits |= 1 << bit;
            }
        }
    }
    plane
}

fn sixel_char(bits: u8) -> char {
    char::from(0x3f + bits)
}
