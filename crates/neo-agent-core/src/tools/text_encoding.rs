pub(crate) const ENCODING_DETECTION_SAMPLE_BYTES: usize = 512;

const MIN_UTF16_ZERO_BYTES: usize = 2;

#[derive(Clone, Copy)]
pub(crate) enum Utf16ByteOrder {
    LittleEndian,
    BigEndian,
}

impl Utf16ByteOrder {
    fn unit(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::LittleEndian => u16::from_le_bytes(bytes),
            Self::BigEndian => u16::from_be_bytes(bytes),
        }
    }
}

pub(crate) struct Utf16Decoder {
    pending_high_surrogate: Option<u16>,
}

impl Utf16Decoder {
    pub(crate) const fn new() -> Self {
        Self {
            pending_high_surrogate: None,
        }
    }

    pub(crate) fn push_unit(&mut self, output: &mut String, unit: u16) {
        if let Some(high) = self.pending_high_surrogate.take() {
            if (0xdc00..=0xdfff).contains(&unit) {
                let scalar =
                    0x1_0000 + (((u32::from(high) - 0xd800) << 10) | (u32::from(unit) - 0xdc00));
                output.push(char::from_u32(scalar).expect("valid UTF-16 surrogate pair"));
                return;
            }
            output.push(char::REPLACEMENT_CHARACTER);
        }

        match unit {
            0xd800..=0xdbff => self.pending_high_surrogate = Some(unit),
            0xdc00..=0xdfff => output.push(char::REPLACEMENT_CHARACTER),
            _ => output
                .push(char::from_u32(u32::from(unit)).expect("non-surrogate UTF-16 unit is valid")),
        }
    }

    pub(crate) fn finish(&mut self, output: &mut String) {
        if self.pending_high_surrogate.take().is_some() {
            output.push(char::REPLACEMENT_CHARACTER);
        }
    }
}

pub(crate) fn decode_text(bytes: &[u8]) -> Option<String> {
    let sample_len = bytes.len().min(ENCODING_DETECTION_SAMPLE_BYTES);
    let (byte_order, bom_len) = detect_utf16_byte_order(&bytes[..sample_len]);
    let Some(byte_order) = byte_order else {
        return std::str::from_utf8(bytes).ok().map(str::to_owned);
    };
    let bytes = &bytes[bom_len..];
    let mut decoder = Utf16Decoder::new();
    let mut output = String::new();
    let mut chunks = bytes.chunks_exact(2);

    for chunk in &mut chunks {
        decoder.push_unit(&mut output, byte_order.unit([chunk[0], chunk[1]]));
    }
    decoder.finish(&mut output);
    if !chunks.remainder().is_empty() {
        output.push(char::REPLACEMENT_CHARACTER);
    }
    Some(output)
}

pub(crate) fn detect_utf16_byte_order(bytes: &[u8]) -> (Option<Utf16ByteOrder>, usize) {
    if bytes.starts_with(&[0xff, 0xfe]) {
        return (Some(Utf16ByteOrder::LittleEndian), 2);
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        return (Some(Utf16ByteOrder::BigEndian), 2);
    }

    let (zeros_at_even, zeros_at_odd) =
        bytes
            .iter()
            .enumerate()
            .fold((0, 0), |(even, odd), (index, byte)| {
                if *byte != 0 {
                    (even, odd)
                } else if index % 2 == 0 {
                    (even + 1, odd)
                } else {
                    (even, odd + 1)
                }
            });

    if zeros_at_even == 0 && zeros_at_odd >= MIN_UTF16_ZERO_BYTES {
        (Some(Utf16ByteOrder::LittleEndian), 0)
    } else if zeros_at_odd == 0 && zeros_at_even >= MIN_UTF16_ZERO_BYTES {
        (Some(Utf16ByteOrder::BigEndian), 0)
    } else {
        (None, 0)
    }
}
