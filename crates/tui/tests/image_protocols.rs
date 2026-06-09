use neo_tui::{
    ImageProtocolError, Iterm2Dimension, Iterm2InlineImageOptions, KittyGraphicsOptions,
    KittyImageFormat, SixelImageOptions, SixelPaletteColor, encode_iterm2_inline_image,
    encode_kitty_graphics, encode_sixel_image,
};

#[test]
fn kitty_graphics_encodes_png_bytes_as_direct_apc_transfer() {
    let encoded = encode_kitty_graphics(
        b"hello",
        &KittyGraphicsOptions::new(KittyImageFormat::Png)
            .with_image_id(42)
            .with_pixel_size(2, 3),
    )
    .expect("valid kitty image sequence");

    assert_eq!(encoded, "\x1b_Ga=T,f=100,t=d,i=42,s=2,v=3;aGVsbG8=\x1b\\");
}

#[test]
fn kitty_graphics_chunks_large_payloads_with_explicit_final_marker() {
    let encoded = encode_kitty_graphics(
        b"abcdef",
        &KittyGraphicsOptions::new(KittyImageFormat::Png).with_chunk_size(4),
    )
    .expect("valid chunked kitty image sequence");

    assert_eq!(
        encoded,
        "\x1b_Ga=T,f=100,t=d,m=1;YWJj\x1b\\\x1b_Gm=0;ZGVm\x1b\\"
    );
    assert_eq!(encoded.matches("\x1b_G").count(), 2);
}

#[test]
fn iterm2_inline_image_encodes_metadata_and_payload_as_osc_1337() {
    let encoded = encode_iterm2_inline_image(
        b"hello",
        &Iterm2InlineImageOptions::new()
            .with_name("neo image")
            .with_width(Iterm2Dimension::Pixels(640))
            .with_height(Iterm2Dimension::Cells(12))
            .with_preserve_aspect_ratio(false),
    )
    .expect("valid iterm2 inline image sequence");

    assert_eq!(
        encoded,
        "\x1b]1337;File=name=bmVvIGltYWdl;width=640px;height=12;preserveAspectRatio=0;inline=1:aGVsbG8=\x07"
    );
}

#[test]
fn sixel_image_encodes_indexed_pixels_as_dcs_sixel_payload() {
    let encoded = encode_sixel_image(
        &[0, 1, 1, 0],
        &SixelImageOptions::new(
            2,
            2,
            vec![
                SixelPaletteColor::rgb_percent(100, 0, 0),
                SixelPaletteColor::rgb_percent(0, 0, 100),
            ],
        ),
    )
    .expect("valid sixel image sequence");

    assert_eq!(
        encoded,
        "\x1bPq\"1;1;2;2#0;2;100;0;0#1;2;0;0;100#0@A$#1A@\x1b\\"
    );
}

#[test]
fn sixel_image_encodes_pixels_across_six_row_bands() {
    let encoded = encode_sixel_image(
        &[0, 0, 0, 0, 0, 0, 0],
        &SixelImageOptions::new(1, 7, vec![SixelPaletteColor::rgb_percent(0, 100, 0)]),
    )
    .expect("valid multi-band sixel image sequence");

    assert_eq!(encoded, "\x1bPq\"1;1;1;7#0;2;0;100;0#0~-#0@\x1b\\");
}

#[test]
fn image_protocol_encoders_reject_empty_payloads_and_invalid_options() {
    assert_eq!(
        encode_kitty_graphics(b"", &KittyGraphicsOptions::new(KittyImageFormat::Png)),
        Err(ImageProtocolError::EmptyImageData)
    );
    assert_eq!(
        encode_kitty_graphics(
            b"hello",
            &KittyGraphicsOptions::new(KittyImageFormat::Png).with_chunk_size(0),
        ),
        Err(ImageProtocolError::InvalidChunkSize)
    );
    assert_eq!(
        encode_iterm2_inline_image(b"", &Iterm2InlineImageOptions::new()),
        Err(ImageProtocolError::EmptyImageData)
    );
    assert_eq!(
        encode_sixel_image(
            &[],
            &SixelImageOptions::new(1, 1, vec![SixelPaletteColor::rgb_percent(0, 0, 0)]),
        ),
        Err(ImageProtocolError::EmptyImageData)
    );
    assert_eq!(
        encode_sixel_image(
            &[0],
            &SixelImageOptions::new(0, 1, vec![SixelPaletteColor::rgb_percent(0, 0, 0)]),
        ),
        Err(ImageProtocolError::InvalidDimension)
    );
    assert_eq!(
        encode_sixel_image(
            &[0, 0],
            &SixelImageOptions::new(1, 1, vec![SixelPaletteColor::rgb_percent(0, 0, 0)]),
        ),
        Err(ImageProtocolError::InvalidPixelDataLength)
    );
    assert_eq!(
        encode_sixel_image(
            &[1],
            &SixelImageOptions::new(1, 1, vec![SixelPaletteColor::rgb_percent(0, 0, 0)]),
        ),
        Err(ImageProtocolError::InvalidColorIndex)
    );
    assert_eq!(
        encode_sixel_image(
            &[0],
            &SixelImageOptions::new(1, 1, vec![SixelPaletteColor::rgb_percent(101, 0, 0)]),
        ),
        Err(ImageProtocolError::InvalidPalette)
    );
}
