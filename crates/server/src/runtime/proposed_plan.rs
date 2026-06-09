const OPEN_TAG: &str = "<proposed_plan>";
const CLOSE_TAG: &str = "</proposed_plan>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProposedPlanSegment {
    Normal(String),
    PlanStart,
    PlanDelta(String),
    PlanEnd,
}

#[derive(Debug)]
pub(super) struct ProposedPlanParser {
    inside_plan: bool,
    detect_tag: bool,
    line_buffer: String,
}

impl Default for ProposedPlanParser {
    fn default() -> Self {
        Self {
            inside_plan: false,
            detect_tag: true,
            line_buffer: String::new(),
        }
    }
}

impl ProposedPlanParser {
    pub(super) fn push_str(&mut self, text: &str) -> Vec<ProposedPlanSegment> {
        let mut segments = Vec::new();
        let mut run = String::new();

        for ch in text.chars() {
            if self.detect_tag {
                if !run.is_empty() {
                    self.push_text(std::mem::take(&mut run), &mut segments);
                }
                self.line_buffer.push(ch);
                if ch == '\n' {
                    self.finish_line(&mut segments);
                    continue;
                }
                let slug = self.line_buffer.trim_start();
                if slug.is_empty() || is_tag_prefix(slug) {
                    continue;
                }
                let buffered = std::mem::take(&mut self.line_buffer);
                self.detect_tag = false;
                self.push_text(buffered, &mut segments);
                continue;
            }

            run.push(ch);
            if ch == '\n' {
                self.push_text(std::mem::take(&mut run), &mut segments);
                self.detect_tag = true;
            }
        }

        if !run.is_empty() {
            self.push_text(run, &mut segments);
        }
        segments
    }

    pub(super) fn finish(&mut self) -> Vec<ProposedPlanSegment> {
        let mut segments = Vec::new();
        if !self.line_buffer.is_empty() {
            let buffered = std::mem::take(&mut self.line_buffer);
            let without_newline = buffered.strip_suffix('\n').unwrap_or(&buffered);
            let slug = without_newline.trim_start().trim_end();

            if slug == OPEN_TAG && !self.inside_plan {
                push_segment(&mut segments, ProposedPlanSegment::PlanStart);
                self.inside_plan = true;
            } else if slug == CLOSE_TAG && self.inside_plan {
                push_segment(&mut segments, ProposedPlanSegment::PlanEnd);
                self.inside_plan = false;
            } else {
                self.push_text(buffered, &mut segments);
            }
        }
        if self.inside_plan {
            self.inside_plan = false;
            push_segment(&mut segments, ProposedPlanSegment::PlanEnd);
        }
        self.detect_tag = true;
        segments
    }

    fn finish_line(&mut self, segments: &mut Vec<ProposedPlanSegment>) {
        let line = std::mem::take(&mut self.line_buffer);
        let without_newline = line.strip_suffix('\n').unwrap_or(&line);
        let slug = without_newline.trim_start().trim_end();

        if slug == OPEN_TAG && !self.inside_plan {
            push_segment(segments, ProposedPlanSegment::PlanStart);
            self.inside_plan = true;
            self.detect_tag = true;
            return;
        }

        if slug == CLOSE_TAG && self.inside_plan {
            push_segment(segments, ProposedPlanSegment::PlanEnd);
            self.inside_plan = false;
            self.detect_tag = true;
            return;
        }

        self.detect_tag = true;
        self.push_text(line, segments);
    }

    fn push_text(&self, text: String, segments: &mut Vec<ProposedPlanSegment>) {
        if self.inside_plan {
            push_segment(segments, ProposedPlanSegment::PlanDelta(text));
        } else {
            push_segment(segments, ProposedPlanSegment::Normal(text));
        }
    }
}

fn is_tag_prefix(slug: &str) -> bool {
    let slug = slug.trim_end();
    OPEN_TAG.starts_with(slug) || CLOSE_TAG.starts_with(slug)
}

fn push_segment(segments: &mut Vec<ProposedPlanSegment>, segment: ProposedPlanSegment) {
    match segment {
        ProposedPlanSegment::Normal(delta) => {
            if delta.is_empty() {
                return;
            }
            if let Some(ProposedPlanSegment::Normal(existing)) = segments.last_mut() {
                existing.push_str(&delta);
                return;
            }
            segments.push(ProposedPlanSegment::Normal(delta));
        }
        ProposedPlanSegment::PlanDelta(delta) => {
            if delta.is_empty() {
                return;
            }
            if let Some(ProposedPlanSegment::PlanDelta(existing)) = segments.last_mut() {
                existing.push_str(&delta);
                return;
            }
            segments.push(ProposedPlanSegment::PlanDelta(delta));
        }
        ProposedPlanSegment::PlanStart => segments.push(ProposedPlanSegment::PlanStart),
        ProposedPlanSegment::PlanEnd => segments.push(ProposedPlanSegment::PlanEnd),
    }
}

#[cfg(test)]
pub(super) fn strip_proposed_plan_blocks(text: &str) -> String {
    let mut parser = ProposedPlanParser::default();
    let mut stripped = parser
        .push_str(text)
        .into_iter()
        .chain(parser.finish())
        .filter_map(|segment| match segment {
            ProposedPlanSegment::Normal(text) => Some(text),
            ProposedPlanSegment::PlanStart
            | ProposedPlanSegment::PlanDelta(_)
            | ProposedPlanSegment::PlanEnd => None,
        })
        .collect::<String>();
    while stripped.contains("\n\n") {
        stripped = stripped.replace("\n\n", "\n");
    }
    stripped
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parser_handles_split_tags() {
        let mut parser = ProposedPlanParser::default();
        let mut segments = parser.push_str("Intro\n<prop");
        segments.extend(parser.push_str("osed_plan>\n- step\n</proposed"));
        segments.extend(parser.push_str("_plan>\nOutro"));
        segments.extend(parser.finish());

        assert_eq!(
            segments,
            vec![
                ProposedPlanSegment::Normal("Intro\n".to_string()),
                ProposedPlanSegment::PlanStart,
                ProposedPlanSegment::PlanDelta("- step\n".to_string()),
                ProposedPlanSegment::PlanEnd,
                ProposedPlanSegment::Normal("Outro".to_string()),
            ]
        );
    }

    #[test]
    fn strip_removes_plan_blocks() {
        let text = "before\n<proposed_plan>\n- step\n</proposed_plan>\nafter";
        assert_eq!(strip_proposed_plan_blocks(text), "before\nafter");
    }

    #[test]
    fn malformed_inline_tag_stays_normal_text() {
        let mut parser = ProposedPlanParser::default();
        let mut segments = parser.push_str("  <proposed_plan> extra\n");
        segments.extend(parser.finish());

        assert_eq!(
            segments,
            vec![ProposedPlanSegment::Normal(
                "  <proposed_plan> extra\n".to_string()
            )]
        );
    }
}
