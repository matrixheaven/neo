use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";

/// Anchor row of every kitty image id in a frame, keyed by image id. The
/// fullscreen diff renderer uses this to detect placements whose screen row
/// changed between frames: kitty keeps drawing a placement until it is
/// explicitly deleted, so a moved image must be deleted and re-emitted.
pub(super) fn collect_kitty_image_rows(lines: &[String]) -> BTreeMap<u32, BTreeSet<usize>> {
    let mut rows: BTreeMap<u32, BTreeSet<usize>> = BTreeMap::new();
    for (row, line) in lines.iter().enumerate() {
        for id in extract_kitty_image_ids(line) {
            rows.entry(id).or_default().insert(row);
        }
    }
    rows
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
