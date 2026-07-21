//! Shell decomposition used for per-segment permission checks.

use tree_sitter::Node;
use tree_sitter::Tree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedCommand {
    words: Vec<String>,
    start_byte: usize,
    end_byte: usize,
}

impl ParsedCommand {
    pub(super) fn words(&self) -> &[String] {
        &self.words
    }

    pub(super) fn start_byte(&self) -> usize {
        self.start_byte
    }

    pub(super) fn end_byte(&self) -> usize {
        self.end_byte
    }
}

/// Decompose a script into literal command words. The public shell utility is
/// the fast path; this module adds the redirect- and compound-aware traversal
/// needed by the policy gates.
pub(super) fn all_commands_from_script(script: &str) -> Option<Vec<ParsedCommand>> {
    let tree = devo_util_shell_command::bash::try_parse_shell(script)?;
    let commands = if let Some(commands) =
        devo_util_shell_command::bash::try_parse_word_only_commands_sequence(&tree, script)
    {
        let nodes = command_nodes(tree.root_node());
        if commands.len() != nodes.len() {
            return None;
        }
        commands
            .into_iter()
            .zip(nodes)
            .map(|(words, node)| ParsedCommand {
                words,
                start_byte: node.start_byte(),
                end_byte: command_effect_end(node),
            })
            .collect()
    } else {
        parse_redirect_aware_commands(&tree, script)?
    };
    if commands.iter().any(|command| {
        matches!(
            classify_shell_dash_c_script(unwrap_wrappers(command.words())),
            ShellDashCScript::Uncertain
        )
    }) {
        // Callers already interpret decomposition failure as Ask. Preserve
        // that fail-closed path when a literal shell command has no reliably
        // positioned `-c` script.
        return None;
    }
    Some(commands)
}

fn parse_redirect_aware_commands(tree: &Tree, script: &str) -> Option<Vec<ParsedCommand>> {
    if tree.root_node().has_error() {
        return None;
    }
    const ALLOWED_NAMED_KINDS: &[&str] = &[
        "program",
        "list",
        "pipeline",
        "command",
        "command_name",
        "word",
        "string",
        "string_content",
        "raw_string",
        "number",
        "concatenation",
        "variable_assignment",
        "variable_name",
        "redirected_statement",
        "file_redirect",
        "file_descriptor",
        "comment",
        "heredoc_redirect",
        "heredoc_start",
        "heredoc_body",
        "heredoc_content",
        "heredoc_end",
        // Compound shell syntax. Only literal `command` descendants are
        // emitted; expansions and substitutions remain absent from this
        // allowlist and therefore fail closed.
        "subshell",
        "compound_statement",
        "function_definition",
        "if_statement",
        "elif_clause",
        "else_clause",
        "for_statement",
        "while_statement",
        "do_group",
        "case_statement",
        "case_item",
        "negated_command",
        "test_command",
        "test_operator",
        "unary_expression",
        "binary_expression",
        "parenthesized_expression",
        "regex",
    ];
    const ALLOWED_TOKENS: &[&str] = &[
        "&&", "||", ";", "|", "|&", "&", "!", "(", ")", "{", "}", "\"", "'", "=", ">", ">>", "<",
        "<<", ">&", "&>", "&>>", "if", "then", "elif", "else", "fi", "for", "select", "in", "do",
        "done", "while", "until", "case", "esac", ";;", ";&", ";;&", "function", "[[", "]]", "[",
        "]", "-a", "-o", "!=", "=~", "==", "<=", ">=", "+", "-", "*", "/", "%", "**", "++", "--",
        "^", ",", "?", ":", "~",
    ];

    let root = tree.root_node();
    let mut stack = vec![root];
    let mut command_nodes = Vec::new();
    while let Some(node) = stack.pop() {
        if node.is_named() {
            if !ALLOWED_NAMED_KINDS.contains(&node.kind()) {
                return None;
            }
            if node.kind() == "command" {
                command_nodes.push(node);
            }
        } else if !ALLOWED_TOKENS.contains(&node.kind()) && !node.kind().trim().is_empty() {
            return None;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    command_nodes.sort_by_key(Node::start_byte);
    command_nodes
        .into_iter()
        .map(|node| parse_plain_command(node, script))
        .collect()
}

fn parse_plain_command(command: Node<'_>, script: &str) -> Option<ParsedCommand> {
    let mut words = Vec::new();
    let mut cursor = command.walk();
    for child in command.named_children(&mut cursor) {
        match child.kind() {
            "variable_assignment" | "file_redirect" | "heredoc_redirect" => {}
            "command_name" => {
                words.push(literal_arg(child.named_child(0)?, script)?);
            }
            "word" | "number" | "string" | "raw_string" | "concatenation" => {
                words.push(literal_arg(child, script)?);
            }
            _ => return None,
        }
    }
    (!words.is_empty()).then_some(ParsedCommand {
        words,
        start_byte: command.start_byte(),
        end_byte: command_effect_end(command),
    })
}

fn command_nodes(root: Node<'_>) -> Vec<Node<'_>> {
    let mut stack = vec![root];
    let mut commands = Vec::new();
    while let Some(node) = stack.pop() {
        if node.kind() == "command" {
            commands.push(node);
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    commands.sort_by_key(Node::start_byte);
    commands
}

fn command_effect_end(mut node: Node<'_>) -> usize {
    let mut end = node.end_byte();
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "redirected_statement" => end = parent.end_byte(),
            "list" | "pipeline" | "program" => break,
            _ => {}
        }
        node = parent;
    }
    end
}

fn literal_arg(node: Node<'_>, script: &str) -> Option<String> {
    let raw = node.utf8_text(script.as_bytes()).ok()?;
    match node.kind() {
        "word" | "number" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .next()
                .is_none()
                .then(|| raw.to_string())
        }
        "raw_string" => raw
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .map(str::to_owned),
        "string" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .all(|child| child.kind() == "string_content")
                .then(|| {
                    raw.strip_prefix('"')
                        .and_then(|value| value.strip_suffix('"'))
                        .unwrap_or(raw)
                        .to_string()
                })
        }
        "concatenation" => {
            let mut result = String::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                result.push_str(&literal_arg(child, script)?);
            }
            (!result.is_empty()).then_some(result)
        }
        _ => None,
    }
}

pub(super) fn unwrap_wrappers(words: &[String]) -> &[String] {
    let mut current = words;
    for _ in 0..8 {
        match strip_wrapper(current) {
            Some(inner) => current = inner,
            None => break,
        }
    }
    current
}

fn strip_wrapper(words: &[String]) -> Option<&[String]> {
    let program = basename(words.first()?);
    let mut index = 1;
    match program {
        "timeout" => {
            while let Some(argument) = words.get(index) {
                if !argument.starts_with('-') {
                    break;
                }
                index += if matches!(argument.as_str(), "-k" | "-s" | "--kill-after" | "--signal") {
                    2
                } else {
                    1
                };
            }
            words.get(index)?;
            index += 1;
        }
        "nice" => {
            while let Some(argument) = words.get(index) {
                if !argument.starts_with('-') {
                    break;
                }
                index += if matches!(argument.as_str(), "-n" | "--adjustment") {
                    2
                } else {
                    1
                };
            }
        }
        "ionice" => {
            while let Some(argument) = words.get(index) {
                if !argument.starts_with('-') {
                    break;
                }
                index += if matches!(
                    argument.as_str(),
                    "-c" | "-n"
                        | "-p"
                        | "-P"
                        | "-u"
                        | "--class"
                        | "--classdata"
                        | "--pid"
                        | "--pgid"
                        | "--uid"
                ) {
                    2
                } else {
                    1
                };
            }
        }
        "chrt" => {
            while words
                .get(index)
                .is_some_and(|argument| argument.starts_with('-'))
            {
                index += 1;
            }
            words.get(index)?;
            index += 1;
        }
        "stdbuf" => {
            while let Some(argument) = words.get(index) {
                if !argument.starts_with('-') {
                    break;
                }
                index += if matches!(argument.as_str(), "-i" | "-o" | "-e") {
                    2
                } else {
                    1
                };
            }
        }
        "env" => index = env_inner_index(words),
        "command" => {
            while let Some(argument) = words.get(index) {
                match argument.as_str() {
                    "--" => {
                        index += 1;
                        break;
                    }
                    "-p" => index += 1,
                    _ => break,
                }
            }
        }
        "exec" => {
            while let Some(argument) = words.get(index) {
                match argument.as_str() {
                    "--" => {
                        index += 1;
                        break;
                    }
                    "-a" => index += 2,
                    "-c" | "-l" => index += 1,
                    _ => break,
                }
            }
        }
        _ => return None,
    }
    words.get(index..).filter(|inner| !inner.is_empty())
}

fn env_inner_index(words: &[String]) -> usize {
    let mut index = 1;
    while let Some(argument) = words.get(index) {
        if argument == "--" {
            return index + 1;
        }
        if argument != "-" && argument.starts_with('-') {
            index += if matches!(
                argument.as_str(),
                "-C" | "--chdir" | "-u" | "--unset" | "-S" | "--split-string"
            ) {
                2
            } else {
                1
            };
        } else if is_env_assignment(argument) {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn is_env_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellDashCScript<'a> {
    NotPresent,
    Script(&'a str),
    Uncertain,
}

pub(super) fn shell_dash_c_script(words: &[String]) -> Option<&str> {
    match classify_shell_dash_c_script(words) {
        ShellDashCScript::Script(script) => Some(script),
        ShellDashCScript::NotPresent | ShellDashCScript::Uncertain => None,
    }
}

pub(super) fn is_external_shell_script_invocation(words: &[String]) -> bool {
    let Some(program) = words.first() else {
        return false;
    };
    if !matches!(basename(program), "bash" | "sh" | "dash" | "zsh" | "ksh")
        || !matches!(
            classify_shell_dash_c_script(words),
            ShellDashCScript::NotPresent
        )
    {
        return false;
    }

    let mut index = 1;
    while let Some(argument) = words.get(index).map(String::as_str) {
        if argument == "--" {
            return words.get(index + 1).is_some();
        }
        if argument == "-" {
            return false;
        }
        if !argument.starts_with('-') || argument == "-c" {
            return true;
        }
        index += if matches!(argument, "-o" | "-O" | "--init-file" | "--rcfile") {
            2
        } else {
            1
        };
    }
    false
}

fn classify_shell_dash_c_script(words: &[String]) -> ShellDashCScript<'_> {
    let Some(program) = words.first() else {
        return ShellDashCScript::NotPresent;
    };
    if !matches!(basename(program), "bash" | "sh" | "dash" | "zsh" | "ksh") {
        return ShellDashCScript::NotPresent;
    }

    let mut index = 1;
    let mut has_dash_c = false;
    while let Some(word) = words.get(index).map(String::as_str) {
        if matches!(word, "--" | "-") {
            return match (has_dash_c, words.get(index + 1)) {
                (true, Some(script)) => ShellDashCScript::Script(script),
                (true, None) => ShellDashCScript::Uncertain,
                (false, _) => ShellDashCScript::NotPresent,
            };
        }
        if let Some(long_option) = word.strip_prefix("--") {
            if has_dash_c {
                return ShellDashCScript::Uncertain;
            }
            index += if matches!(long_option, "init-file" | "rcfile") {
                2
            } else {
                1
            };
            continue;
        }
        let Some(cluster) = word
            .strip_prefix('-')
            .or_else(|| word.strip_prefix('+'))
            .filter(|cluster| !cluster.is_empty())
        else {
            return if has_dash_c {
                ShellDashCScript::Script(word)
            } else {
                ShellDashCScript::NotPresent
            };
        };

        let enables_dash_c = word.starts_with('-') && cluster.contains('c');
        has_dash_c |= enables_dash_c;
        let option_value_count = cluster
            .bytes()
            .filter(|option| matches!(option, b'o' | b'O'))
            .count();
        index += 1;
        for _ in 0..option_value_count {
            let Some(value) = words.get(index).map(String::as_str) else {
                return if has_dash_c {
                    ShellDashCScript::Uncertain
                } else {
                    ShellDashCScript::NotPresent
                };
            };
            if matches!(value, "--" | "-") {
                return if has_dash_c {
                    ShellDashCScript::Uncertain
                } else {
                    ShellDashCScript::NotPresent
                };
            }
            index += 1;
        }
    }
    if has_dash_c {
        ShellDashCScript::Uncertain
    } else {
        ShellDashCScript::NotPresent
    }
}

pub(super) fn basename(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_literal_commands_in_subshells_and_conditionals() {
        fn words(cmd: &ParsedCommand) -> Vec<String> {
            cmd.words().to_vec()
        }
        fn run(script: &str) -> Option<Vec<Vec<String>>> {
            let parsed = all_commands_from_script(script)?;
            Some(parsed.iter().map(words).collect())
        }
        assert_eq!(run("(rm /x)"), Some(vec![vec!["rm".into(), "/x".into()]]));
        assert_eq!(
            run("if true; then rm /x; fi"),
            Some(vec![vec!["true".into()], vec!["rm".into(), "/x".into()],])
        );
    }

    #[test]
    fn parses_literal_commands_in_loops_functions_and_background_pipelines() {
        fn words(cmd: &ParsedCommand) -> Vec<String> {
            cmd.words().to_vec()
        }
        fn run(script: &str) -> Option<Vec<Vec<String>>> {
            let parsed = all_commands_from_script(script)?;
            Some(parsed.iter().map(words).collect())
        }
        assert_eq!(
            run("for item in a b; do rm /x; done"),
            Some(vec![vec!["rm".to_string(), "/x".to_string()]])
        );
        assert_eq!(
            run("cleanup() { rm /x; }; cleanup"),
            Some(vec![
                vec!["rm".to_string(), "/x".to_string()],
                vec!["cleanup".to_string()],
            ])
        );
        assert_eq!(
            run("while true; do rm /x; done"),
            Some(vec![
                vec!["true".to_string()],
                vec!["rm".to_string(), "/x".to_string()],
            ])
        );
        assert_eq!(
            run("sleep 1 & echo safe | rm /x"),
            Some(vec![
                vec!["sleep".to_string(), "1".to_string()],
                vec!["echo".to_string(), "safe".to_string()],
                vec!["rm".to_string(), "/x".to_string()],
            ])
        );
    }

    #[test]
    fn rejects_ambiguous_expansions_in_compound_commands() {
        for script in [
            "(rm \"$TARGET\")",
            "if true; then echo $(rm /x); fi",
            "for item in \"$@\"; do rm \"$item\"; done",
        ] {
            assert_eq!(all_commands_from_script(script), None, "{script}");
        }
    }

    #[test]
    fn extracts_dash_c_script_around_value_taking_options() {
        for (command, script) in [
            (
                words(&["bash", "-O", "extglob", "-o", "pipefail", "-lc", "rm /x"]),
                "rm /x",
            ),
            (
                words(&["bash", "-lc", "-O", "extglob", "-o", "pipefail", "rm /x"]),
                "rm /x",
            ),
            (words(&["bash", "-co", "pipefail", "rm /x"]), "rm /x"),
            (words(&["bash", "-Oc", "extglob", "rm /x"]), "rm /x"),
            (words(&["bash", "-c", "--", "rm /x"]), "rm /x"),
        ] {
            assert_eq!(shell_dash_c_script(&command), Some(script), "{command:?}");
        }
    }

    #[test]
    fn rejects_missing_or_unpositionable_dash_c_scripts() {
        for command in [
            words(&["bash", "-c"]),
            words(&["bash", "-c", "-O"]),
            words(&["bash", "-c", "-o", "pipefail"]),
            words(&["bash", "-O", "extglob", "-c"]),
            words(&["bash", "-c", "--"]),
        ] {
            assert_eq!(shell_dash_c_script(&command), None, "{command:?}");
            assert_eq!(
                classify_shell_dash_c_script(&command),
                ShellDashCScript::Uncertain,
                "{command:?}"
            );
        }
        for command in [
            words(&["bash", "-o", "-c", "rm /x"]),
            words(&["bash", "--", "-c", "rm /x"]),
        ] {
            assert_eq!(shell_dash_c_script(&command), None, "{command:?}");
            assert_eq!(
                classify_shell_dash_c_script(&command),
                ShellDashCScript::NotPresent,
                "{command:?}"
            );
        }
        assert_eq!(all_commands_from_script("bash -c -O"), None);
    }
}
