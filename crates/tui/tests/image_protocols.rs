use neo_tui::{
    ImageProtocolError, Iterm2Dimension, Iterm2InlineImageOptions, KittyGraphicsOptions,
    KittyImageFormat, encode_iterm2_inline_image, encode_kitty_graphics,
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
}
