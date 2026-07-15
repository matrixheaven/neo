use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

#[must_use]
pub(super) fn stable_prefix_len(markdown: &str) -> usize {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut events = Parser::new_ext(markdown, options).into_offset_iter();
    if events.reference_definitions().iter().next().is_some() {
        return 0;
    }

    let mut depth = 0usize;
    let mut previous_block_end = None;
    let mut stable_boundary = 0usize;

    for (event, range) in &mut events {
        match event {
            Event::Start(tag) => {
                if contains_reference_link(&tag) {
                    return 0;
                }
                if depth == 0 && is_top_level_block_start(&tag) {
                    update_boundary(
                        markdown,
                        previous_block_end,
                        range.start,
                        &mut stable_boundary,
                    );
                }
                depth = depth.saturating_add(1);
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                if depth == 0 && is_top_level_block_end(tag) {
                    previous_block_end = Some(range.end);
                }
            }
            Event::Rule if depth == 0 => {
                update_boundary(
                    markdown,
                    previous_block_end,
                    range.start,
                    &mut stable_boundary,
                );
                previous_block_end = Some(range.end);
            }
            Event::FootnoteReference(_) => return 0,
            _ => {}
        }
    }

    stable_boundary
}

fn update_boundary(
    markdown: &str,
    previous_block_end: Option<usize>,
    next_block_start: usize,
    stable_boundary: &mut usize,
) {
    let Some(previous_block_end) = previous_block_end else {
        return;
    };
    let separator_start = previous_block_end.saturating_sub(1);
    if separator_start <= next_block_start
        && has_blank_line(&markdown[separator_start..next_block_start])
    {
        *stable_boundary = next_block_start;
    }
}

fn has_blank_line(source: &str) -> bool {
    let bytes = source.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let mut next = index + 1;
        while next < bytes.len() && matches!(bytes[next], b' ' | b'\t' | b'\r') {
            next += 1;
        }
        if bytes.get(next) == Some(&b'\n') {
            return true;
        }
    }
    false
}

fn contains_reference_link(tag: &Tag<'_>) -> bool {
    let link_type = match tag {
        Tag::Link { link_type, .. } | Tag::Image { link_type, .. } => *link_type,
        _ => return false,
    };
    matches!(
        link_type,
        LinkType::Reference
            | LinkType::ReferenceUnknown
            | LinkType::Collapsed
            | LinkType::CollapsedUnknown
            | LinkType::Shortcut
            | LinkType::ShortcutUnknown
    )
}

fn is_top_level_block_start(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::Table(_)
            | Tag::MetadataBlock(_)
    )
}

fn is_top_level_block_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::Table
            | TagEnd::MetadataBlock(_)
    )
}

#[cfg(test)]
mod tests {
    use super::stable_prefix_len;

    #[test]
    fn complete_plain_paragraph_is_stable_but_open_markdown_tail_is_not() {
        assert_eq!(
            stable_prefix_len("first paragraph\n\nsecond"),
            "first paragraph\n\n".len()
        );
        assert_eq!(stable_prefix_len("```rust\nfn main() {}\n"), 0);
        assert_eq!(stable_prefix_len("[link][target]\n\n[target]: /later"), 0);
    }
}
