#[must_use]
pub fn sanitize_shell_output(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut output = String::with_capacity(raw.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            0x1b => {
                index = skip_escape_sequence(bytes, index + 1);
            }
            byte if byte < 0x20 && byte != b'\n' && byte != b'\t' => {
                index += 1;
            }
            _ => {
                let next = next_char_boundary(raw, index + 1);
                output.push_str(&raw[index..next]);
                index = next;
            }
        }
    }
    output
}

#[must_use]
pub fn split_sanitized_shell_lines(stdout: &str, stderr: &str) -> Vec<String> {
    let combined = format!(
        "{}{}",
        sanitize_shell_output(stdout),
        sanitize_shell_output(stderr)
    );
    combined.lines().map(str::to_owned).collect()
}

fn skip_escape_sequence(bytes: &[u8], index: usize) -> usize {
    let Some(&kind) = bytes.get(index) else {
        return index;
    };
    match kind {
        b']' => skip_osc(bytes, index + 1),
        b'[' => skip_csi(bytes, index + 1),
        _ => index + 1,
    }
}

fn skip_osc(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        if bytes[index] == 0x07 {
            return index + 1;
        }
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
            return index + 2;
        }
        index += 1;
    }
    index
}

fn skip_csi(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        if (0x40..=0x7e).contains(&bytes[index]) {
            return index + 1;
        }
        index += 1;
    }
    index
}

fn next_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}
