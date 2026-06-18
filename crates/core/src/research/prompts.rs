//! Prompt construction for the deep-research workflow.
//!
//! The runtime keeps stage instructions static and passes user/request context as
//! separate escaped message blocks. That keeps template rendering predictable and
//! avoids accidentally injecting user text into system prompts.

use std::borrow::Cow;

use chrono::Local;

const SYSTEM_TEMPLATE: &str = include_str!("../../prompts/research/system.md");
const CLARIFY_TEMPLATE: &str = include_str!("../../prompts/research/clarify.md");
const RESEARCH_BRIEF_TEMPLATE: &str = include_str!("../../prompts/research/research_brief.md");
const SUPERVISOR_TEMPLATE: &str = include_str!("../../prompts/research/supervisor.md");
const RESEARCHER_TEMPLATE: &str = include_str!("../../prompts/research/researcher.md");
const SUBAGENT_TEMPLATE: &str = include_str!("../../prompts/research/subagent.md");
const COMPRESS_TEMPLATE: &str = include_str!("../../prompts/research/compress.md");
const FINAL_REPORT_TEMPLATE: &str = include_str!("../../prompts/research/final_report.md");
const SUMMARIZE_WEBPAGE_TEMPLATE: &str =
    include_str!("../../prompts/research/summarize_webpage.md");
const MAX_TASKS_PLACEHOLDER: &str = "{{ max_tasks }}";
const MAX_ITERATIONS_PLACEHOLDER: &str = "{{ max_iterations }}";
const MAX_SUMMARY_CHARS_PLACEHOLDER: &str = "{{ max_summary_chars }}";

pub fn today_string() -> String {
    Local::now().format("%B %d, %Y").to_string()
}

pub fn timezone_string() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string())
}

pub fn system() -> String {
    SYSTEM_TEMPLATE.trim_end().to_string()
}

pub fn environment_context(current_date: &str, timezone: &str, cwd: &str) -> String {
    let current_date = escape_xml_text(current_date);
    let timezone = escape_xml_text(timezone);
    let cwd = escape_xml_text(cwd);
    format!(
        "<research_environment>\n<current_date>{current_date}</current_date>\n<timezone>{timezone}</timezone>\n<cwd>{cwd}</cwd>\n</research_environment>"
    )
}

pub fn clarification_context(question: &str, answer: &str) -> String {
    let question = escape_xml_text(question);
    let answer = escape_xml_text(answer);
    format!(
        "<clarification_context>\n<question>{question}</question>\n<answer>{answer}</answer>\n</clarification_context>"
    )
}

pub fn research_brief_context(research_brief: &str) -> String {
    tagged_block("research_brief", research_brief)
}

pub fn research_topic_context(research_topic: &str) -> String {
    tagged_block("research_topic", research_topic)
}

pub fn research_notes_context(research_notes: &str) -> String {
    tagged_block("research_notes", research_notes)
}

pub fn webpage_summaries_context(webpage_summaries: &str) -> String {
    tagged_block("webpage_summaries", webpage_summaries)
}

pub fn findings_context(findings: &str) -> String {
    tagged_block("findings", findings)
}

pub fn source_context(source_url: &str, source_title: &str, webpage_content: &str) -> String {
    let source_url = escape_xml_text(source_url);
    let source_title = escape_xml_text(source_title);
    let webpage_content = escape_xml_text(webpage_content);
    format!(
        "<source>\n<url>{source_url}</url>\n<title>{source_title}</title>\n<content>{webpage_content}</content>\n</source>"
    )
}

pub fn clarify() -> String {
    CLARIFY_TEMPLATE.trim_end().to_string()
}

pub fn research_brief() -> String {
    RESEARCH_BRIEF_TEMPLATE.trim_end().to_string()
}

pub fn supervisor(max_tasks: usize) -> String {
    let max_tasks = max_tasks.to_string();
    render(SUPERVISOR_TEMPLATE, &[(MAX_TASKS_PLACEHOLDER, &max_tasks)])
}

pub fn researcher(max_iterations: usize) -> String {
    let max_iterations = max_iterations.to_string();
    render(
        RESEARCHER_TEMPLATE,
        &[(MAX_ITERATIONS_PLACEHOLDER, &max_iterations)],
    )
}

pub fn subagent() -> String {
    SUBAGENT_TEMPLATE.trim_end().to_string()
}

pub fn compress() -> String {
    COMPRESS_TEMPLATE.trim_end().to_string()
}

pub fn final_report() -> String {
    FINAL_REPORT_TEMPLATE.trim_end().to_string()
}

pub fn summarize_webpage(max_summary_chars: usize) -> String {
    let max_summary_chars = max_summary_chars.to_string();
    render(
        SUMMARIZE_WEBPAGE_TEMPLATE,
        &[(MAX_SUMMARY_CHARS_PLACEHOLDER, &max_summary_chars)],
    )
}

fn render(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut rendered = template.to_string();
    for (placeholder, value) in replacements {
        rendered = rendered.replace(placeholder, value);
    }
    rendered
}

fn tagged_block(tag: &str, text: &str) -> String {
    let text = escape_xml_text(text);
    format!("<{tag}>\n{text}\n</{tag}>")
}

fn escape_xml_text(text: &str) -> Cow<'_, str> {
    if !text
        .as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        return Cow::Borrowed(text);
    }

    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn user_request_is_not_rendered_into_stage_prompt() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: stage prompts do not template-inject the original user request.
        let prompt = clarify();

        assert!(!prompt.contains("research &lt;tag&gt; &amp; &quot;quoted&quot;"));
        assert!(!prompt.contains("research <tag> & \"quoted\""));
        assert!(!prompt.contains("{{ messages }}"));
        assert!(!prompt.contains("{{ date }}"));
    }

    #[test]
    fn environment_context_escapes_date_timezone_and_cwd() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: research environment is a separate user-role context block.
        let context = environment_context("2026-06-17", "Asia/<Shanghai>", "/tmp/a&b");

        assert_eq!(
            context,
            "<research_environment>\n<current_date>2026-06-17</current_date>\n<timezone>Asia/&lt;Shanghai&gt;</timezone>\n<cwd>/tmp/a&amp;b</cwd>\n</research_environment>"
        );
    }

    #[test]
    fn clarification_context_escapes_user_text() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: clarification state is rendered separately from stage prompts.
        let context = clarification_context("Which <scope>?", "Use A & B");

        assert!(context.contains("Which &lt;scope&gt;?"));
        assert!(context.contains("Use A &amp; B"));
    }

    #[test]
    fn supervisor_prompt_renders_max_tasks() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: supervisor prompt renders the configured task cap.
        let prompt = supervisor(4);

        assert!(prompt.contains("at most 4 tasks"));
        assert!(!prompt.contains("{{ max_tasks }}"));
        assert!(!prompt.contains("Compare A and B"));
    }

    #[test]
    fn summarize_webpage_prompt_renders_threshold_only() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: oversized local web fetch summary prompts render caps without source data.
        let prompt = summarize_webpage(8000);

        assert!(prompt.contains("under 8000 characters"));
        assert!(!prompt.contains("{{ webpage_content }}"));
    }

    #[test]
    fn researcher_prompt_renders_iteration_limit_only() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: researcher prompts receive stage config while brief/topic stay in messages.
        let prompt = researcher(5);

        assert!(prompt.contains("after 5 search/fetch iterations"));
        assert!(!prompt.contains("{{ research_brief }}"));
    }

    #[test]
    fn subagent_prompt_is_static_worker_instruction() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: delegated research workers receive a dedicated static prompt.
        let prompt = subagent();

        assert!(prompt.contains("Stage: delegated deep research worker."));
        assert!(prompt.contains("not a final user-facing"));
        assert!(prompt.contains("Do not write files"));
        assert!(prompt.contains("assistant text only"));
        assert!(!prompt.contains("{{"));
    }

    #[test]
    fn context_helpers_escape_research_artifacts() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: generated research artifacts are separate escaped context blocks.
        let context = research_notes_context("web_search <input>");

        assert_eq!(
            context,
            "<research_notes>\nweb_search &lt;input&gt;\n</research_notes>"
        );
    }

    #[test]
    fn xml_escaping_borrows_plain_text() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: large context blocks without XML delimiters avoid an extra copy.
        let escaped = escape_xml_text("plain research notes");

        assert!(matches!(escaped, Cow::Borrowed("plain research notes")));
    }

    #[test]
    fn renderer_preserves_unmentioned_template_text() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: prompt placeholder replacement leaves unrelated template text intact.
        assert_eq!(
            render("Hello {{ name }}", &[("{{ name }}", "Devo")]),
            "Hello Devo"
        );
    }
}
