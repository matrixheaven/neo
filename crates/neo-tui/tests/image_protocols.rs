use neo_tui::shell::InlineImageRenderCache;
use neo_tui::terminal_image::{
    ImageDisplayOptions, ImageProtocolError, ImageProtocolPreference, ImageRenderPolicy,
    ImageSource, InlineImage, Iterm2Dimension, Iterm2InlineImageOptions, KittyGraphicsOptions,
    KittyImageFormat, NegotiatedImageProtocol, SixelImageOptions, SixelPaletteColor,
    TerminalImageCapabilities, encode_iterm2_inline_image, encode_kitty_graphics,
    encode_sixel_image,
};
use neo_tui::transcript::InlineImageRender;

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

#[test]
fn image_protocol_errors_have_stable_messages() {
    let messages = [
        (
            ImageProtocolError::EmptyImageData,
            "image data must not be empty",
        ),
        (
            ImageProtocolError::InvalidChunkSize,
            "kitty chunk size must be greater than zero",
        ),
        (
            ImageProtocolError::InvalidColorIndex,
            "sixel pixel data contains a palette index out of range",
        ),
        (
            ImageProtocolError::InvalidDimension,
            "image dimensions must be greater than zero",
        ),
        (
            ImageProtocolError::InvalidPalette,
            "sixel palette must not be empty and RGB percentage values must be <= 100",
        ),
        (
            ImageProtocolError::InvalidPixelDataLength,
            "sixel pixel data length must exactly match image width multiplied by height",
        ),
    ];

    for (error, message) in messages {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn image_protocol_auto_negotiates_available_terminal_protocol() {
    assert_eq!(
        ImageRenderPolicy::new(ImageProtocolPreference::Auto, false)
            .negotiate(TerminalImageCapabilities::default().with_kitty(true)),
        NegotiatedImageProtocol::Kitty
    );
    assert_eq!(
        ImageRenderPolicy::new(ImageProtocolPreference::Auto, false)
            .negotiate(TerminalImageCapabilities::default().with_iterm2(true)),
        NegotiatedImageProtocol::Iterm2
    );
    assert_eq!(
        ImageRenderPolicy::new(ImageProtocolPreference::Auto, false)
            .negotiate(TerminalImageCapabilities::default().with_sixel(true)),
        NegotiatedImageProtocol::None
    );
    assert_eq!(
        ImageRenderPolicy::new(ImageProtocolPreference::None, true).negotiate(
            TerminalImageCapabilities::default()
                .with_kitty(true)
                .with_iterm2(true)
                .with_sixel(true)
        ),
        NegotiatedImageProtocol::None
    );
}

#[test]
fn image_render_policy_keeps_remote_images_metadata_only_by_default() {
    let policy = ImageRenderPolicy::new(ImageProtocolPreference::Kitty, false);
    let remote = InlineImage::remote_url(
        "img-remote",
        "image/png",
        "https://example.test/private.png",
        Some("private chart"),
    );

    let rendered = policy.render_inline_image(
        &remote,
        TerminalImageCapabilities::default().with_kitty(true),
        &ImageDisplayOptions::bounded(1, 1),
    );

    assert_eq!(rendered.protocol, NegotiatedImageProtocol::None);
    assert_eq!(
        rendered.metadata,
        "[image: image/png url=https://example.test/private.png alt=\"private chart\"]"
    );
    assert!(rendered.escape_sequence.is_none());
    assert_eq!(remote.size_bytes(), None);
}

#[test]
fn image_render_policy_renders_local_or_base64_payloads_with_selected_protocol() {
    let policy = ImageRenderPolicy::new(ImageProtocolPreference::Kitty, false);
    let local = InlineImage::bytes(
        "img-local",
        "image/png",
        [137, 80, 78, 71],
        Some("generated plot"),
        ImageSource::Generated,
    );

    let rendered = policy.render_inline_image(
        &local,
        TerminalImageCapabilities::default().with_kitty(true),
        &ImageDisplayOptions::bounded(1, 1),
    );

    assert_eq!(local.size_bytes(), Some(4));
    assert_eq!(rendered.protocol, NegotiatedImageProtocol::Kitty);
    assert!(
        rendered
            .metadata
            .contains("[image: image/png generated 4 bytes alt=\"generated plot\"]")
    );
    assert!(
        rendered
            .escape_sequence
            .as_deref()
            .is_some_and(|sequence| sequence.starts_with("\x1b_G"))
    );
}

#[test]
fn terminal_image_thumbnail_uses_bounded_kitty_cell_dimensions() {
    let policy = ImageRenderPolicy::new(ImageProtocolPreference::Kitty, false);
    let image = InlineImage::bytes(
        "img-wide",
        "image/png",
        [137, 80, 78, 71],
        Some("wide screenshot"),
        ImageSource::Base64,
    );

    let rendered = policy.render_inline_image(
        &image,
        TerminalImageCapabilities::default().with_kitty(true),
        &ImageDisplayOptions::thumbnail(1_184, 650, "[image #1 (1184x650)]"),
    );

    assert_eq!(rendered.protocol, NegotiatedImageProtocol::Kitty);
    assert_eq!(rendered.lines.len(), 12);
    assert!(rendered.lines[0].contains("\x1b_G"));
    assert!(rendered.lines[0].contains("c=22"));
    assert!(rendered.lines[0].contains("r=12"));
    assert!(rendered.lines.iter().skip(1).all(String::is_empty));
}

#[test]
fn terminal_image_thumbnail_uses_bounded_iterm2_cell_dimensions() {
    let policy = ImageRenderPolicy::new(ImageProtocolPreference::Iterm2, false);
    let image = InlineImage::bytes(
        "img-wide",
        "image/png",
        [137, 80, 78, 71],
        Some("wide screenshot"),
        ImageSource::Base64,
    );

    let rendered = policy.render_inline_image(
        &image,
        TerminalImageCapabilities::default().with_iterm2(true),
        &ImageDisplayOptions::thumbnail(1_184, 650, "[image #1 (1184x650)]"),
    );

    assert_eq!(rendered.protocol, NegotiatedImageProtocol::Iterm2);
    assert_eq!(rendered.lines.len(), 1);
    assert!(rendered.lines[0].contains("\x1b]1337;File="));
    assert!(rendered.lines[0].contains("width=22"));
    assert!(rendered.lines[0].contains("height=12"));
}

#[test]
fn terminal_image_thumbnail_falls_back_without_inline_support_or_space() {
    let policy = ImageRenderPolicy::new(ImageProtocolPreference::Auto, false);
    let image = InlineImage::bytes(
        "img-local",
        "image/png",
        [137, 80, 78, 71],
        None::<String>,
        ImageSource::Base64,
    );

    let rendered = policy.render_inline_image(
        &image,
        TerminalImageCapabilities::default().with_sixel(true),
        &ImageDisplayOptions::thumbnail(640, 480, "[image #1 (640x480)]").with_max_cols(0),
    );

    assert_eq!(rendered.protocol, NegotiatedImageProtocol::None);
    assert_eq!(rendered.lines, vec!["[image #1 (640x480)]"]);
    assert!(rendered.escape_sequence.is_none());
}

#[test]
fn terminal_image_thumbnail_does_not_fake_sixel_for_encoded_image_bytes() {
    let policy = ImageRenderPolicy::new(ImageProtocolPreference::Sixel, false);
    let image = InlineImage::bytes(
        "img-local",
        "image/png",
        [137, 80, 78, 71],
        None::<String>,
        ImageSource::Base64,
    );

    let rendered = policy.render_inline_image(
        &image,
        TerminalImageCapabilities::default().with_sixel(true),
        &ImageDisplayOptions::thumbnail(640, 480, "[image #1 (640x480)]"),
    );

    assert_eq!(rendered.protocol, NegotiatedImageProtocol::None);
    assert_eq!(rendered.lines, vec!["[image #1 (640x480)]"]);
    assert!(rendered.escape_sequence.is_none());
}

#[test]
fn inline_image_render_cache_suppresses_duplicates_until_redraw_reset() {
    let renders = vec![
        InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-1".to_owned(),
        },
        InlineImageRender {
            id: "image-2".to_owned(),
            escape_sequence: "kitty-2".to_owned(),
        },
    ];
    let mut cache = InlineImageRenderCache::default();

    assert_eq!(cache.take_pending(renders.clone()), renders);
    assert!(
        cache
            .take_pending(vec![InlineImageRender {
                id: "image-1".to_owned(),
                escape_sequence: "kitty-1".to_owned(),
            }])
            .is_empty()
    );

    cache.reset_for_full_redraw();
    assert_eq!(
        cache.take_pending(vec![InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-1".to_owned(),
        }]),
        vec![InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-1".to_owned(),
        }]
    );
}

#[test]
fn inline_image_render_cache_reemits_changed_image_payload_without_full_reset() {
    let mut cache = InlineImageRenderCache::default();

    assert_eq!(
        cache.take_pending(vec![InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-old".to_owned(),
        }]),
        vec![InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-old".to_owned(),
        }]
    );
    assert_eq!(
        cache.take_pending(vec![InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-new".to_owned(),
        }]),
        vec![InlineImageRender {
            id: "image-1".to_owned(),
            escape_sequence: "kitty-new".to_owned(),
        }]
    );
}
