use std::collections::BTreeSet;
use std::fmt::Write as _;

const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";

pub(super) fn collect_kitty_image_ids(lines: &[String]) -> BTreeSet<u32> {
    lines
        .iter()
        .flat_map(|line| extract_kitty_image_ids(line))
        .collect()
}

fn extract_kitty_image_ids(line: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut rest = line;
    while let Some(sequence_start) = rest.find(KITTY_SEQUENCE_PREFIX) {
        rest = &rest[sequence_start + KITTY_SEQUENCE_PREFIX.len()..];
        let Some(params_end) = rest.find(';') else {
            break;
        };
        for param in rest[..params_end].split(',') {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            if key == "i"
                && let Ok(id) = value.parse::<u32>()
                && id > 0
            {
                ids.push(id);
            }
        }
        rest = &rest[params_end + 1..];
    }
    ids
}

pub(super) fn delete_kitty_images(ids: &BTreeSet<u32>) -> String {
    let mut output = String::new();
    for id in ids {
        let _ = write!(output, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
    }
    output
}
