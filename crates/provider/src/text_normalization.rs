//! Splits provider-emitted tagged reasoning from assistant text.
//!
//! The parser keeps partial tags in `pending` so streaming chunks can end in the
//! middle of `<think>`-style markers without leaking the marker text to clients.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaggedTextFragment {
    Text(String),
    Reasoning(String),
}

#[derive(Debug, Default)]
pub(crate) struct TaggedTextParser {
    pending: String,
    in_reasoning: bool,
}

impl TaggedTextParser {
    pub(crate) fn consume(&mut self, chunk: &str) -> Vec<TaggedTextFragment> {
        self.pending.push_str(chunk);
        let mut fragments = Vec::new();

        loop {
            let tags = if self.in_reasoning {
                &CLOSE_TAGS[..]
            } else {
                &OPEN_TAGS[..]
            };

            if let Some((start, tag)) = find_first_tag(&self.pending, tags) {
                if start > 0 {
                    let prefix = take_prefix(&mut self.pending, start);
                    fragments.push(fragment_for_mode(self.in_reasoning, prefix));
                }
                self.pending.drain(..tag.len());
                self.in_reasoning = !self.in_reasoning;
                continue;
            }

            let partial_start = earliest_partial_start(&self.pending, tags);
            if let Some(partial_start) = partial_start {
                if partial_start > 0 {
                    let prefix = take_prefix(&mut self.pending, partial_start);
                    fragments.push(fragment_for_mode(self.in_reasoning, prefix));
                }
            } else if !self.pending.is_empty() {
                fragments.push(fragment_for_mode(
                    self.in_reasoning,
                    std::mem::take(&mut self.pending),
                ));
            }
            break;
        }

        fragments
    }

    pub(crate) fn finish(&mut self) -> Vec<TaggedTextFragment> {
        if self.pending.is_empty() {
            Vec::new()
        } else {
            vec![fragment_for_mode(
                self.in_reasoning,
                std::mem::take(&mut self.pending),
            )]
        }
    }
}

fn take_prefix(text: &mut String, end: usize) -> String {
    let suffix = text.split_off(end);
    std::mem::replace(text, suffix)
}

pub(crate) fn split_tagged_text(text: &str) -> (String, Vec<String>) {
    if !OPEN_TAGS.iter().any(|tag| text.contains(tag)) {
        return (text.to_string(), Vec::new());
    }

    let mut parser = TaggedTextParser::default();
    let mut assistant_text = String::with_capacity(text.len());
    let mut reasoning = Vec::new();

    for fragment in parser.consume(text).into_iter().chain(parser.finish()) {
        match fragment {
            TaggedTextFragment::Text(text) => assistant_text.push_str(&text),
            TaggedTextFragment::Reasoning(text) => reasoning.push(text),
        }
    }

    (assistant_text, reasoning)
}

const OPEN_TAGS: [&str; 2] = ["<think>", "<|begin_of_box|>"];
const CLOSE_TAGS: [&str; 2] = ["</think>", "<|end_of_box|>"];

fn fragment_for_mode(in_reasoning: bool, text: String) -> TaggedTextFragment {
    if in_reasoning {
        TaggedTextFragment::Reasoning(text)
    } else {
        TaggedTextFragment::Text(text)
    }
}

fn find_first_tag<'a>(text: &str, tags: &'a [&str]) -> Option<(usize, &'a str)> {
    tags.iter()
        .filter_map(|tag| text.find(tag).map(|index| (index, *tag)))
        .min_by_key(|(index, _)| *index)
}

fn earliest_partial_start(text: &str, tags: &[&str]) -> Option<usize> {
    for (start, _) in text.char_indices() {
        let suffix = &text[start..];
        if tags
            .iter()
            .any(|tag| tag.starts_with(suffix) && suffix.len() < tag.len())
        {
            return Some(start);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{TaggedTextFragment, TaggedTextParser, split_tagged_text};

    #[test]
    fn split_tagged_text_extracts_think_blocks() {
        let (assistant, reasoning) = split_tagged_text("before<think>plan</think>after");

        assert_eq!(assistant, "beforeafter");
        assert_eq!(reasoning, vec!["plan"]);
    }

    #[test]
    fn split_tagged_text_extracts_box_blocks() {
        let (assistant, reasoning) =
            split_tagged_text("<|begin_of_box|>reasoning<|end_of_box|>final");

        assert_eq!(assistant, "final");
        assert_eq!(reasoning, vec!["reasoning"]);
    }

    #[test]
    fn parser_handles_split_tags_across_chunks() {
        let mut parser = TaggedTextParser::default();
        let mut fragments = Vec::new();
        fragments.extend(parser.consume("before<th"));
        fragments.extend(parser.consume("ink>plan</th"));
        fragments.extend(parser.consume("ink>after"));
        fragments.extend(parser.finish());

        assert_eq!(
            fragments,
            vec![
                TaggedTextFragment::Text("before".into()),
                TaggedTextFragment::Reasoning("plan".into()),
                TaggedTextFragment::Text("after".into()),
            ]
        );
    }

    #[test]
    fn parser_does_not_slice_inside_utf8_characters() {
        let mut parser = TaggedTextParser::default();

        let fragments = parser.consume("现在");

        assert_eq!(fragments, vec![TaggedTextFragment::Text("现在".into())]);
    }
}
