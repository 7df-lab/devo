use chrono::Local;

const WEB_SEARCH_PROMPT_TEMPLATE: &str = include_str!("websearch.txt");

pub(crate) fn web_search_prompt() -> String {
    render_web_search_prompt(&Local::now().format("%B %Y").to_string())
}

pub(crate) fn render_web_search_prompt(current_month_year: &str) -> String {
    WEB_SEARCH_PROMPT_TEMPLATE.replace("{{currentMonthYear}}", current_month_year)
}

#[cfg(test)]
mod tests {
    use super::render_web_search_prompt;

    #[test]
    fn web_search_prompt_renders_current_month_year() {
        let prompt = render_web_search_prompt("June 2026");

        assert!(prompt.contains("The current month is June 2026."));
        assert!(prompt.contains("Source attribution:"));
        assert!(prompt.contains("[Title](URL)"));
        assert!(prompt.contains("If your answer does not use retrieved information"));
        assert!(prompt.contains("Cite only sources actually retrieved and used."));
        assert!(!prompt.contains("Sources:"));
        assert!(!prompt.contains("MUST include"));
        assert!(!prompt.contains("{{currentMonthYear}}"));
    }
}
