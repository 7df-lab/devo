use crate::shell_command::bash::extract_bash_command;
use crate::shell_command::bash::try_parse_shell;
use crate::shell_command::bash::try_parse_word_only_commands_sequence;
use crate::shell_command::powershell::extract_powershell_command;
use devo_protocol::parse_command::ParsedCommand;
use shlex::split as shlex_split;
use shlex::try_join as shlex_try_join;
use std::path::PathBuf;

pub fn shlex_join(tokens: &[String]) -> String {
    shlex_try_join(tokens.iter().map(String::as_str))
        .unwrap_or_else(|_| "<command included NUL byte>".to_string())
}

/// Extracts the shell and script from a command, regardless of platform
pub fn extract_shell_command(command: &[String]) -> Option<(&str, &str)> {
    extract_bash_command(command).or_else(|| extract_powershell_command(command))
}

/// DO NOT REVIEW THIS CODE BY HAND
/// This parsing code is quite complex and not easy to hand-modify.
/// The easiest way to iterate is to add unit tests and iterate on the implementation with tests.
/// To encourage this, the tests have been put directly below this function rather than at the bottom of the
///
/// Parses metadata out of an arbitrary command.
/// These commands are model driven and could include just about anything.
/// The parsing is slightly lossy due to the ~infinite expressiveness of an arbitrary command.
/// The goal of the parsed metadata is to be able to provide the user with a human readable gis
/// of what it is doing.
pub fn parse_command(command: &[String]) -> Vec<ParsedCommand> {
    // Parse and then collapse consecutive duplicate commands to avoid redundant summaries.
    let parsed = parse_command_impl(command);
    let mut deduped: Vec<ParsedCommand> = Vec::with_capacity(parsed.len());
    for cmd in parsed.into_iter() {
        if deduped.last().is_some_and(|prev| prev == &cmd) {
            continue;
        }
        deduped.push(cmd);
    }
    if deduped
        .iter()
        .any(|cmd| matches!(cmd, ParsedCommand::Unknown { .. }))
    {
        vec![single_unknown_for_command(command)]
    } else {
        deduped
    }
}

fn single_unknown_for_command(command: &[String]) -> ParsedCommand {
    if let Some((_, shell_command)) = extract_shell_command(command) {
        ParsedCommand::Unknown {
            cmd: shell_command.to_string(),
        }
    } else {
        ParsedCommand::Unknown {
            cmd: shlex_join(command),
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use std::string::ToString;

    fn shlex_split_safe(s: &str) -> Vec<String> {
        shlex_split(s).unwrap_or_else(|| s.split_whitespace().map(ToString::to_string).collect())
    }

    fn vec_str(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    fn unknown(cmd: &str) -> ParsedCommand {
        ParsedCommand::Unknown {
            cmd: cmd.to_string(),
        }
    }
    fn search(cmd: &str, query: &str, path: Option<&str>) -> ParsedCommand {
        ParsedCommand::Search {
            cmd: cmd.to_string(),
            query: Some(query.to_string()),
            path: path.map(str::to_string),
        }
    }
    fn list(cmd: &str, path: Option<&str>) -> ParsedCommand {
        ParsedCommand::ListFiles {
            cmd: cmd.to_string(),
            path: path.map(str::to_string),
        }
    }
    fn read(cmd: &str, name: &str, path: &str) -> ParsedCommand {
        ParsedCommand::Read {
            cmd: cmd.to_string(),
            name: name.to_string(),
            path: PathBuf::from(path),
        }
    }

    enum Argv<'a> {
        Shlex(&'a str),
        BashLc(&'a str),
        ZshLc(&'a str),
        BashC(&'a str),
        Tokens(&'a [&'a str]),
    }

    impl Argv<'_> {
        fn to_vec(&self) -> Vec<String> {
            match *self {
                Argv::Shlex(s) => shlex_split_safe(s),
                Argv::BashLc(s) => vec_str(&["bash", "-lc", s]),
                Argv::ZshLc(s) => vec_str(&["zsh", "-lc", s]),
                Argv::BashC(s) => vec_str(&["bash", "-c", s]),
                Argv::Tokens(args) => vec_str(args),
            }
        }
    }

    #[test]
    fn parse_command_table() {
        let cases: &[(&str, Argv<'_>, Vec<ParsedCommand>)] = &[
            (
                "git_status",
                Argv::Tokens(&["git", "status"]),
                vec![unknown("git status")],
            ),
            (
                "git_grep",
                Argv::Shlex("git grep TODO src"),
                vec![search("git grep TODO src", "TODO", Some("src"))],
            ),
            (
                "git_grep_l",
                Argv::Shlex("git grep -l TODO src"),
                vec![search("git grep -l TODO src", "TODO", Some("src"))],
            ),
            (
                "git_ls_files",
                Argv::Shlex("git ls-files"),
                vec![list("git ls-files", None)],
            ),
            (
                "git_ls_files_src",
                Argv::Shlex("git ls-files src"),
                vec![list("git ls-files src", Some("src"))],
            ),
            (
                "git_ls_files_exclude",
                Argv::Shlex("git ls-files --exclude target src"),
                vec![list("git ls-files --exclude target src", Some("src"))],
            ),
            (
                "git_pipe_wc",
                Argv::BashLc("git status | wc -l"),
                vec![unknown("git status | wc -l")],
            ),
            (
                "bash_redirect",
                Argv::BashLc("echo foo > bar"),
                vec![unknown("echo foo > bar")],
            ),
            (
                "complex_bash_head",
                Argv::BashLc(
                    "rg --version && node -v && pnpm -v && rg --files | wc -l && rg --files | head -n 40",
                ),
                vec![unknown(
                    "rg --version && node -v && pnpm -v && rg --files | wc -l && rg --files | head -n 40",
                )],
            ),
            (
                "rg_navigate",
                Argv::BashLc(r#"rg -n "navigate-to-route" -S"#),
                vec![search(
                    "rg -n navigate-to-route -S",
                    "navigate-to-route",
                    None,
                )],
            ),
            (
                "rg_todo_pipe_head",
                Argv::BashLc(r#"rg -n "BUG|FIXME|TODO|XXX|HACK" -S | head -n 200"#),
                vec![search(
                    "rg -n 'BUG|FIXME|TODO|XXX|HACK' -S",
                    "BUG|FIXME|TODO|XXX|HACK",
                    None,
                )],
            ),
            (
                "rg_files_path_pipe",
                Argv::BashLc("rg --files webview/src | sed -n"),
                vec![list("rg --files webview/src", Some("webview"))],
            ),
            (
                "rg_files_head",
                Argv::BashLc("rg --files | head -n 50"),
                vec![list("rg --files", None)],
            ),
            (
                "rg_l",
                Argv::Shlex("rg -l TODO src"),
                vec![search("rg -l TODO src", "TODO", Some("src"))],
            ),
            (
                "rg_files_with_matches",
                Argv::Shlex("rg --files-with-matches TODO src"),
                vec![search(
                    "rg --files-with-matches TODO src",
                    "TODO",
                    Some("src"),
                )],
            ),
            (
                "rg_L",
                Argv::Shlex("rg -L TODO src"),
                vec![search("rg -L TODO src", "TODO", Some("src"))],
            ),
            (
                "rg_files_without_match",
                Argv::Shlex("rg --files-without-match TODO src"),
                vec![search(
                    "rg --files-without-match TODO src",
                    "TODO",
                    Some("src"),
                )],
            ),
            (
                "rga_l",
                Argv::Shlex("rga -l TODO src"),
                vec![search("rga -l TODO src", "TODO", Some("src"))],
            ),
            (
                "cat",
                Argv::BashLc("cat webview/README.md"),
                vec![read(
                    "cat webview/README.md",
                    "README.md",
                    "webview/README.md",
                )],
            ),
            (
                "zsh_cat",
                Argv::ZshLc("cat README.md"),
                vec![read("cat README.md", "README.md", "README.md")],
            ),
            (
                "bat",
                Argv::BashLc("bat --theme TwoDark README.md"),
                vec![read(
                    "bat --theme TwoDark README.md",
                    "README.md",
                    "README.md",
                )],
            ),
            (
                "batcat",
                Argv::BashLc("batcat README.md"),
                vec![read("batcat README.md", "README.md", "README.md")],
            ),
            (
                "less",
                Argv::BashLc("less -p TODO README.md"),
                vec![read("less -p TODO README.md", "README.md", "README.md")],
            ),
            (
                "more",
                Argv::BashLc("more README.md"),
                vec![read("more README.md", "README.md", "README.md")],
            ),
            (
                "cd_cat",
                Argv::Shlex("cd foo && cat foo.txt"),
                vec![read("cat foo.txt", "foo.txt", "foo/foo.txt")],
            ),
            (
                "cd_double_dash",
                Argv::Shlex("cd -- -weird && cat foo.txt"),
                vec![read("cat foo.txt", "foo.txt", "-weird/foo.txt")],
            ),
            (
                "cd_last_operand",
                Argv::Shlex("cd dir1 dir2 && cat foo.txt"),
                vec![read("cat foo.txt", "foo.txt", "dir2/foo.txt")],
            ),
            (
                "bash_cd_bar",
                Argv::Shlex("bash -lc 'cd foo && bar'"),
                vec![unknown("cd foo && bar")],
            ),
            (
                "bash_cd_cat",
                Argv::Shlex("bash -lc 'cd foo && cat foo.txt'"),
                vec![read("cat foo.txt", "foo.txt", "foo/foo.txt")],
            ),
            (
                "ls_pipe",
                Argv::BashLc("ls -la | sed -n '1,120p'"),
                vec![list("ls -la", None)],
            ),
            (
                "eza",
                Argv::Shlex("eza --color=always src"),
                vec![list("eza '--color=always' src", Some("src"))],
            ),
            (
                "exa",
                Argv::Shlex("exa -I target ."),
                vec![list("exa -I target .", Some("."))],
            ),
            (
                "tree",
                Argv::Shlex("tree -L 2 src"),
                vec![list("tree -L 2 src", Some("src"))],
            ),
            (
                "du",
                Argv::Shlex("du -d 2 ."),
                vec![list("du -d 2 .", Some("."))],
            ),
            (
                "head_n",
                Argv::BashLc("head -n 50 Cargo.toml"),
                vec![read("head -n 50 Cargo.toml", "Cargo.toml", "Cargo.toml")],
            ),
            (
                "head_file",
                Argv::BashLc("head Cargo.toml"),
                vec![read("head Cargo.toml", "Cargo.toml", "Cargo.toml")],
            ),
            (
                "cat_sed",
                Argv::BashLc("cat tui/Cargo.toml | sed -n '1,200p'"),
                vec![read(
                    "cat tui/Cargo.toml | sed -n '1,200p'",
                    "Cargo.toml",
                    "tui/Cargo.toml",
                )],
            ),
            (
                "tail_plus",
                Argv::BashLc("tail -n +522 README.md"),
                vec![read("tail -n +522 README.md", "README.md", "README.md")],
            ),
            (
                "tail_n",
                Argv::BashLc("tail -n 30 README.md"),
                vec![read("tail -n 30 README.md", "README.md", "README.md")],
            ),
            (
                "tail_file",
                Argv::BashLc("tail README.md"),
                vec![read("tail README.md", "README.md", "README.md")],
            ),
            (
                "npm_run",
                Argv::Tokens(&["npm", "run", "build"]),
                vec![unknown("npm run build")],
            ),
            (
                "grep_recursive_dot",
                Argv::Tokens(&["grep", "-R", "DEVO_SANDBOX_ENV_VAR", "-n", "."]),
                vec![search(
                    "grep -R DEVO_SANDBOX_ENV_VAR -n .",
                    "DEVO_SANDBOX_ENV_VAR",
                    Some("."),
                )],
            ),
            (
                "grep_recursive_file",
                Argv::Tokens(&[
                    "grep",
                    "-R",
                    "DEVO_SANDBOX_ENV_VAR",
                    "-n",
                    "core/src/spawn.rs",
                ]),
                vec![search(
                    "grep -R DEVO_SANDBOX_ENV_VAR -n core/src/spawn.rs",
                    "DEVO_SANDBOX_ENV_VAR",
                    Some("spawn.rs"),
                )],
            ),
            (
                "egrep",
                Argv::Shlex("egrep -R TODO src"),
                vec![search("egrep -R TODO src", "TODO", Some("src"))],
            ),
            (
                "fgrep",
                Argv::Shlex("fgrep -l TODO src"),
                vec![search("fgrep -l TODO src", "TODO", Some("src"))],
            ),
            (
                "grep_l",
                Argv::Shlex("grep -l TODO src"),
                vec![search("grep -l TODO src", "TODO", Some("src"))],
            ),
            (
                "grep_files_with_matches",
                Argv::Shlex("grep --files-with-matches TODO src"),
                vec![search(
                    "grep --files-with-matches TODO src",
                    "TODO",
                    Some("src"),
                )],
            ),
            (
                "grep_L",
                Argv::Shlex("grep -L TODO src"),
                vec![search("grep -L TODO src", "TODO", Some("src"))],
            ),
            (
                "grep_files_without_match",
                Argv::Shlex("grep --files-without-match TODO src"),
                vec![search(
                    "grep --files-without-match TODO src",
                    "TODO",
                    Some("src"),
                )],
            ),
            (
                "grep_query_slashes",
                Argv::Shlex("grep -R src/main.rs -n ."),
                vec![search("grep -R src/main.rs -n .", "src/main.rs", Some("."))],
            ),
            (
                "grep_backtick",
                Argv::Shlex("grep -R COD`EX_SANDBOX -n"),
                vec![search(
                    "grep -R 'COD`EX_SANDBOX' -n",
                    "COD`EX_SANDBOX",
                    None,
                )],
            ),
            (
                "cd_rg_files",
                Argv::Shlex("cd devo && rg --files"),
                vec![list("rg --files", None)],
            ),
            (
                "cd_rg_pipe",
                Argv::BashLc(
                    r#"cd /Users/pakrym/code/devo && rg -n "devo_api" devo -S | head -n 50"#,
                ),
                vec![search("rg -n devo_api devo -S", "devo_api", Some("devo"))],
            ),
            (
                "nl_sed",
                Argv::BashLc("nl -ba core/src/parse_command.rs | sed -n '1200,1720p'"),
                vec![read(
                    "nl -ba core/src/parse_command.rs | sed -n '1200,1720p'",
                    "parse_command.rs",
                    "core/src/parse_command.rs",
                )],
            ),
            (
                "sed_n",
                Argv::BashLc("sed -n '2000,2200p' tui/src/history_cell.rs"),
                vec![read(
                    "sed -n '2000,2200p' tui/src/history_cell.rs",
                    "history_cell.rs",
                    "tui/src/history_cell.rs",
                )],
            ),
            (
                "awk_file",
                Argv::BashLc("awk '{print $1}' Cargo.toml"),
                vec![read(
                    "awk '{print $1}' Cargo.toml",
                    "Cargo.toml",
                    "Cargo.toml",
                )],
            ),
            (
                "printf_cat",
                Argv::BashLc(
                    r#"printf "\n===== ansi-escape/Cargo.toml =====\n"; cat -- ansi-escape/Cargo.toml"#,
                ),
                vec![read(
                    "cat -- ansi-escape/Cargo.toml",
                    "Cargo.toml",
                    "ansi-escape/Cargo.toml",
                )],
            ),
            (
                "yes_rg",
                Argv::BashLc("yes | rg --files"),
                vec![list("rg --files", None)],
            ),
            (
                "sed_nl_pipeline",
                Argv::Shlex(
                    "sed -n '260,640p' exec/src/event_processor_with_human_output.rs | nl -ba",
                ),
                vec![read(
                    "sed -n '260,640p' exec/src/event_processor_with_human_output.rs",
                    "event_processor_with_human_output.rs",
                    "exec/src/event_processor_with_human_output.rs",
                )],
            ),
            (
                "rg_spaces",
                Argv::Shlex("yes | rg -n 'foo bar' -S"),
                vec![search("rg -n 'foo bar' -S", "foo bar", None)],
            ),
            (
                "ls_glob",
                Argv::Shlex("ls -I '*.test.js'"),
                vec![list("ls -I '*.test.js'", None)],
            ),
            (
                "true_then_rg",
                Argv::Shlex("true && rg --files"),
                vec![list("rg --files", None)],
            ),
            (
                "rg_then_true",
                Argv::Shlex("rg --files && true"),
                vec![list("rg --files", None)],
            ),
            (
                "true_inside_bash",
                Argv::BashLc("true && rg --files"),
                vec![list("rg --files", None)],
            ),
            (
                "rg_or_true",
                Argv::BashLc("rg --files || true"),
                vec![list("rg --files", None)],
            ),
            (
                "win_path",
                Argv::Shlex(r#"cat "pkg\src\main.rs""#),
                vec![read(
                    r#"cat "pkg\\src\\main.rs""#,
                    "main.rs",
                    r#"pkg\src\main.rs"#,
                )],
            ),
            (
                "head_n50",
                Argv::Shlex("bash -lc 'head -n50 Cargo.toml'"),
                vec![read("head -n50 Cargo.toml", "Cargo.toml", "Cargo.toml")],
            ),
            (
                "bash_c_pipeline",
                Argv::BashC("rg --files | head -n 1"),
                vec![list("rg --files", None)],
            ),
            (
                "tail_nplus",
                Argv::Shlex("bash -lc 'tail -n+10 README.md'"),
                vec![read("tail -n+10 README.md", "README.md", "README.md")],
            ),
            (
                "grep_todo",
                Argv::Shlex("grep -R TODO src"),
                vec![search("grep -R TODO src", "TODO", Some("src"))],
            ),
            (
                "ag",
                Argv::Shlex("ag TODO src"),
                vec![search("ag TODO src", "TODO", Some("src"))],
            ),
            (
                "ack",
                Argv::Shlex("ack TODO src"),
                vec![search("ack TODO src", "TODO", Some("src"))],
            ),
            (
                "pt",
                Argv::Shlex("pt TODO src"),
                vec![search("pt TODO src", "TODO", Some("src"))],
            ),
            (
                "rga",
                Argv::Shlex("rga TODO src"),
                vec![search("rga TODO src", "TODO", Some("src"))],
            ),
            (
                "ag_l",
                Argv::Shlex("ag -l TODO src"),
                vec![search("ag -l TODO src", "TODO", Some("src"))],
            ),
            (
                "ack_l",
                Argv::Shlex("ack -l TODO src"),
                vec![search("ack -l TODO src", "TODO", Some("src"))],
            ),
            (
                "pt_l",
                Argv::Shlex("pt -l TODO src"),
                vec![search("pt -l TODO src", "TODO", Some("src"))],
            ),
            (
                "rg_equals_flags",
                Argv::Shlex("rg --colors=never -n foo src"),
                vec![search("rg '--colors=never' -n foo src", "foo", Some("src"))],
            ),
            (
                "cat_double_dash",
                Argv::Shlex("cat -- ./-strange-file-name"),
                vec![read(
                    "cat -- ./-strange-file-name",
                    "-strange-file-name",
                    "./-strange-file-name",
                )],
            ),
            (
                "sed_range_file",
                Argv::Shlex("sed -n '12,20p' Cargo.toml"),
                vec![read(
                    "sed -n '12,20p' Cargo.toml",
                    "Cargo.toml",
                    "Cargo.toml",
                )],
            ),
            (
                "drop_nl",
                Argv::Shlex("rg --files | nl -ba"),
                vec![list("rg --files", None)],
            ),
            (
                "ls_time_style",
                Argv::Shlex("ls --time-style=long-iso ./dist"),
                vec![list("ls '--time-style=long-iso' ./dist", Some("."))],
            ),
            (
                "fd_type",
                Argv::Shlex("fd -t f src/"),
                vec![list("fd -t f src/", Some("src"))],
            ),
            (
                "fd_query",
                Argv::Shlex("fd main src"),
                vec![search("fd main src", "main", Some("src"))],
            ),
            (
                "find_name",
                Argv::Shlex("find . -name '*.rs'"),
                vec![search("find . -name '*.rs'", "*.rs", Some("."))],
            ),
            (
                "find_type",
                Argv::Shlex("find src -type f"),
                vec![list("find src -type f", Some("src"))],
            ),
            (
                "bin_bash_sed",
                Argv::Shlex("/bin/bash -lc 'sed -n '1,10p' Cargo.toml'"),
                vec![read(
                    "sed -n '1,10p' Cargo.toml",
                    "Cargo.toml",
                    "Cargo.toml",
                )],
            ),
            (
                "bin_zsh_sed",
                Argv::Shlex("/bin/zsh -lc 'sed -n '1,10p' Cargo.toml'"),
                vec![read(
                    "sed -n '1,10p' Cargo.toml",
                    "Cargo.toml",
                    "Cargo.toml",
                )],
            ),
            (
                "powershell",
                Argv::Tokens(&["powershell", "-Command", "Get-ChildItem"]),
                vec![unknown("Get-ChildItem")],
            ),
            (
                "pwsh",
                Argv::Tokens(&["pwsh", "-NoProfile", "-c", "Write-Host hi"]),
                vec![unknown("Write-Host hi")],
            ),
        ];
        for (name, argv, expected) in cases {
            assert_eq!(parse_command(&argv.to_vec()), *expected, "{name}");
        }
    }

    #[test]
    fn parse_command_edge_cases_table() {
        let inner = r#"rg -l QkBindingController presentation/src/main/java | xargs perl -pi -e 's/QkBindingController/QkController/g'"#;
        assert_eq!(
            parse_command(&vec_str(&["bash", "-lc", inner])),
            vec![unknown(inner)]
        );
        let command = shlex_split_safe(inner);
        assert_eq!(
            parse_command(&command),
            vec![unknown(&shlex_join(&command))]
        );
        let helper = shlex_split_safe("rg --files | nl -ba | foo");
        assert_eq!(parse_command(&helper), vec![unknown(&shlex_join(&helper))]);
        let python_cases: &[(&str, Vec<ParsedCommand>)] = &[
            (
                r#"python -c "import os; print(os.listdir('.'))""#,
                vec![list(
                    &shlex_join(&shlex_split_safe(
                        r#"python -c "import os; print(os.listdir('.'))""#,
                    )),
                    None,
                )],
            ),
            (
                r#"python3 -c "import glob; print(glob.glob('*.rs'))""#,
                vec![list(
                    &shlex_join(&shlex_split_safe(
                        r#"python3 -c "import glob; print(glob.glob('*.rs'))""#,
                    )),
                    None,
                )],
            ),
            (
                r#"python -c "print('hello')""#,
                vec![unknown(&shlex_join(&shlex_split_safe(
                    r#"python -c "print('hello')""#,
                )))],
            ),
        ];
        for (script, expected) in python_cases {
            assert_eq!(parse_command(&vec_str(&["bash", "-lc", script])), *expected);
        }
        let command = if cfg!(windows) {
            r"C:\windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        } else {
            "/usr/local/bin/powershell.exe"
        };
        assert_eq!(
            parse_command(&vec_str(&[command, "-NoProfile", "-c", "Write-Host hi"])),
            vec![unknown("Write-Host hi")]
        );
    }

    #[test]
    fn small_formatting_table() {
        let always = ["wc", "tr", "cut", "sort", "uniq", "xargs", "tee", "column"];
        for cmd in always {
            assert!(is_small_formatting_command(&shlex_split_safe(cmd)), "{cmd}");
            assert!(
                is_small_formatting_command(&shlex_split_safe(&format!("{cmd} -x"))),
                "{cmd} -x"
            );
        }
        let cases: &[(&str, bool)] = &[
            ("awk '{print $1}'", true),
            ("awk '{print $1}' Cargo.toml", false),
            ("awk -f script.awk Cargo.toml", false),
            ("head", true),
            ("head -n 40", true),
            ("head -n 40 file.txt", false),
            ("head file.txt", false),
            ("tail", true),
            ("tail -n +10", true),
            ("tail -n +10 file.txt", false),
            ("tail -n 30", true),
            ("tail -n 30 file.txt", false),
            ("tail -c 30", true),
            ("tail -c +10", true),
            ("tail file.txt", false),
            ("sed", true),
            ("sed -n 10p", true),
            ("sed -n 10p file.txt", false),
            ("sed -n -e 10p file.txt", false),
            ("sed -n 10p -- file.txt", false),
            ("sed -n 1,200p file.txt", false),
            ("sed -n p file.txt", true),
            ("sed -n +10p file.txt", true),
        ];
        for (cmd, expected) in cases {
            assert_eq!(
                is_small_formatting_command(&shlex_split_safe(cmd)),
                *expected,
                "{cmd}"
            );
        }
        assert!(!is_small_formatting_command(&Vec::new()));
    }
}

pub fn parse_command_impl(command: &[String]) -> Vec<ParsedCommand> {
    if let Some(commands) = parse_shell_lc_commands(command) {
        return commands;
    }

    if let Some((_, script)) = extract_powershell_command(command) {
        return vec![ParsedCommand::Unknown {
            cmd: script.to_string(),
        }];
    }

    let normalized = normalize_tokens(command);

    let parts = if contains_connectors(&normalized) {
        split_on_connectors(&normalized)
    } else {
        vec![normalized]
    };

    // Preserve left-to-right execution order for all commands, including bash -c/-lc
    // so summaries reflect the order they will run.

    // Map each pipeline segment to its parsed summary, tracking `cd` to compute paths.
    let mut commands: Vec<ParsedCommand> = Vec::new();
    let mut cwd: Option<String> = None;
    for tokens in &parts {
        if let Some((head, tail)) = tokens.split_first()
            && head == "cd"
        {
            if let Some(dir) = cd_target(tail) {
                cwd = Some(match &cwd {
                    Some(base) => join_paths(base, &dir),
                    None => dir.clone(),
                });
            }
            continue;
        }
        let parsed = summarize_main_tokens(tokens);
        let parsed = match parsed {
            ParsedCommand::Read { cmd, name, path } => {
                if let Some(base) = &cwd {
                    let full = join_paths(base, &path.to_string_lossy());
                    ParsedCommand::Read {
                        cmd,
                        name,
                        path: PathBuf::from(full),
                    }
                } else {
                    ParsedCommand::Read { cmd, name, path }
                }
            }
            other => other,
        };
        commands.push(parsed);
    }

    while let Some(next) = simplify_once(&commands) {
        commands = next;
    }

    commands
}

fn simplify_once(commands: &[ParsedCommand]) -> Option<Vec<ParsedCommand>> {
    if commands.len() <= 1 {
        return None;
    }

    // echo ... && ...rest => ...rest
    if let ParsedCommand::Unknown { cmd } = &commands[0]
        && shlex_split(cmd).is_some_and(|t| t.first().map(String::as_str) == Some("echo"))
    {
        return Some(commands[1..].to_vec());
    }

    // cd foo && [any command] => [any command] (keep non-cd when a cd is followed by something)
    if let Some(idx) = commands.iter().position(|pc| match pc {
        ParsedCommand::Unknown { cmd } => {
            shlex_split(cmd).is_some_and(|t| t.first().map(String::as_str) == Some("cd"))
        }
        _ => false,
    }) && commands.len() > idx + 1
    {
        let mut out = Vec::with_capacity(commands.len() - 1);
        out.extend_from_slice(&commands[..idx]);
        out.extend_from_slice(&commands[idx + 1..]);
        return Some(out);
    }

    // cmd || true => cmd
    if let Some(idx) = commands
        .iter()
        .position(|pc| matches!(pc, ParsedCommand::Unknown { cmd } if cmd == "true"))
    {
        let mut out = Vec::with_capacity(commands.len() - 1);
        out.extend_from_slice(&commands[..idx]);
        out.extend_from_slice(&commands[idx + 1..]);
        return Some(out);
    }

    // nl -[any_flags] && ...rest => ...rest
    if let Some(idx) = commands.iter().position(|pc| match pc {
        ParsedCommand::Unknown { cmd } => {
            if let Some(tokens) = shlex_split(cmd) {
                tokens.first().is_some_and(|s| s.as_str() == "nl")
                    && tokens.iter().skip(1).all(|t| t.starts_with('-'))
            } else {
                false
            }
        }
        _ => false,
    }) {
        let mut out = Vec::with_capacity(commands.len() - 1);
        out.extend_from_slice(&commands[..idx]);
        out.extend_from_slice(&commands[idx + 1..]);
        return Some(out);
    }

    None
}

/// Validates that this is a `sed -n 123,123p` command.
fn is_valid_sed_n_arg(arg: Option<&str>) -> bool {
    let s = match arg {
        Some(s) => s,
        None => return false,
    };
    let core = match s.strip_suffix('p') {
        Some(rest) => rest,
        None => return false,
    };
    let parts: Vec<&str> = core.split(',').collect();
    match parts.as_slice() {
        [num] => !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()),
        [a, b] => {
            !a.is_empty()
                && !b.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

fn sed_read_path(args: &[String]) -> Option<String> {
    let args_no_connector = trim_at_connector(args);
    if !args_no_connector.iter().any(|arg| arg == "-n") {
        return None;
    }
    let mut has_range_script = false;
    let mut i = 0;
    while i < args_no_connector.len() {
        let arg = &args_no_connector[i];
        if matches!(arg.as_str(), "-e" | "--expression") {
            if is_valid_sed_n_arg(args_no_connector.get(i + 1).map(String::as_str)) {
                has_range_script = true;
            }
            i += 2;
            continue;
        }
        if matches!(arg.as_str(), "-f" | "--file") {
            i += 2;
            continue;
        }
        i += 1;
    }
    if !has_range_script {
        has_range_script = args_no_connector
            .iter()
            .any(|arg| !arg.starts_with('-') && is_valid_sed_n_arg(Some(arg)));
    }
    if !has_range_script {
        return None;
    }
    let candidates = skip_flag_values(&args_no_connector, &["-e", "-f", "--expression", "--file"]);
    let non_flags: Vec<String> = candidates
        .into_iter()
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect();
    match non_flags.as_slice() {
        [] => None,
        [first, rest @ ..] if is_valid_sed_n_arg(Some(first)) => rest.first().cloned(),
        [first, ..] => Some(first.clone()),
    }
}

/// Normalize a command by:
/// - Removing `yes`/`no`/`bash -c`/`bash -lc`/`zsh -c`/`zsh -lc` prefixes.
/// - Splitting on `|` and `&&`/`||`/`;
fn normalize_tokens(cmd: &[String]) -> Vec<String> {
    match cmd {
        [first, pipe, rest @ ..] if (first == "yes" || first == "y") && pipe == "|" => {
            // Do not re-shlex already-tokenized input; just drop the prefix.
            rest.to_vec()
        }
        [first, pipe, rest @ ..] if (first == "no" || first == "n") && pipe == "|" => {
            // Do not re-shlex already-tokenized input; just drop the prefix.
            rest.to_vec()
        }
        [shell, flag, script]
            if (shell == "bash" || shell == "zsh") && (flag == "-c" || flag == "-lc") =>
        {
            shlex_split(script).unwrap_or_else(|| vec![shell.clone(), flag.clone(), script.clone()])
        }
        _ => cmd.to_vec(),
    }
}

fn contains_connectors(tokens: &[String]) -> bool {
    tokens
        .iter()
        .any(|t| t == "&&" || t == "||" || t == "|" || t == ";")
}

fn split_on_connectors(tokens: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for t in tokens {
        if t == "&&" || t == "||" || t == "|" || t == ";" {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(t.clone());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn trim_at_connector(tokens: &[String]) -> Vec<String> {
    let idx = tokens
        .iter()
        .position(|t| t == "|" || t == "&&" || t == "||" || t == ";")
        .unwrap_or(tokens.len());
    tokens[..idx].to_vec()
}

/// Shorten a path to the last component, excluding `build`/`dist`/`node_modules`/`src`.
/// It also pulls out a useful path from a directory such as:
/// - webview/src -> webview
/// - foo/src/ -> foo
/// - packages/app/node_modules/ -> app
fn parsed_read(main_cmd: &[String], path: &str) -> ParsedCommand {
    ParsedCommand::Read {
        cmd: shlex_join(main_cmd),
        name: short_display_path(path),
        path: PathBuf::from(path),
    }
}

fn read_from_operand(main_cmd: &[String], tail: &[String], flags: &[&str]) -> ParsedCommand {
    if let Some(path) = single_non_flag_operand(tail, flags) {
        parsed_read(main_cmd, &path)
    } else {
        ParsedCommand::Unknown {
            cmd: shlex_join(main_cmd),
        }
    }
}

fn read_from_n_line_command(
    main_cmd: &[String],
    tail: &[String],
    allow_plus_prefix: bool,
) -> ParsedCommand {
    let has_valid_n = match tail.split_first() {
        Some((first, rest)) if first == "-n" => rest.first().is_some_and(|n| {
            let digits = if allow_plus_prefix {
                n.strip_prefix('+').unwrap_or(n)
            } else {
                n.as_str()
            };
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }),
        Some((first, _)) if first.starts_with("-n") => {
            let value = &first[2..];
            let digits = if allow_plus_prefix {
                value.strip_prefix('+').unwrap_or(value)
            } else {
                value
            };
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    };
    if has_valid_n {
        let mut candidates: Vec<&String> = Vec::new();
        let mut index = 0;
        while index < tail.len() {
            if index == 0 && tail[index] == "-n" && index + 1 < tail.len() {
                let n = &tail[index + 1];
                let digits = if allow_plus_prefix {
                    n.strip_prefix('+').unwrap_or(n)
                } else {
                    n.as_str()
                };
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    index += 2;
                    continue;
                }
            }
            candidates.push(&tail[index]);
            index += 1;
        }
        if let Some(path) = candidates.into_iter().find(|p| !p.starts_with('-')) {
            return parsed_read(main_cmd, path);
        }
    }
    if let [path] = tail
        && !path.starts_with('-')
    {
        return parsed_read(main_cmd, path);
    }
    ParsedCommand::Unknown {
        cmd: shlex_join(main_cmd),
    }
}

fn short_display_path(path: &str) -> String {
    // Normalize separators and drop any trailing slash for display.
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    let mut parts = trimmed.split('/').rev().filter(|p| {
        !p.is_empty() && *p != "build" && *p != "dist" && *p != "node_modules" && *p != "src"
    });
    parts
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| trimmed.to_string())
}

// Skip values consumed by specific flags and ignore --flag=value style arguments.
fn skip_flag_values<'a>(args: &'a [String], flags_with_vals: &[&str]) -> Vec<&'a String> {
    let mut out: Vec<&'a String> = Vec::new();
    let mut skip_next = false;
    for (i, a) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--" {
            // From here on, everything is positional operands; push the rest and break.
            for rest in &args[i + 1..] {
                out.push(rest);
            }
            break;
        }
        if a.starts_with("--") && a.contains('=') {
            // --flag=value form: treat as a flag taking a value; skip entirely.
            continue;
        }
        if flags_with_vals.contains(&a.as_str()) {
            // This flag consumes the next argument as its value.
            if i + 1 < args.len() {
                skip_next = true;
            }
            continue;
        }
        out.push(a);
    }
    out
}

fn first_non_flag_operand(args: &[String], flags_with_vals: &[&str]) -> Option<String> {
    positional_operands(args, flags_with_vals)
        .into_iter()
        .next()
        .cloned()
}

fn single_non_flag_operand(args: &[String], flags_with_vals: &[&str]) -> Option<String> {
    let mut operands = positional_operands(args, flags_with_vals).into_iter();
    let first = operands.next()?;
    if operands.next().is_some() {
        return None;
    }
    Some(first.clone())
}

fn positional_operands<'a>(args: &'a [String], flags_with_vals: &[&str]) -> Vec<&'a String> {
    let mut out = Vec::new();
    let mut after_double_dash = false;
    let mut skip_next = false;
    for (i, arg) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if after_double_dash {
            out.push(arg);
            continue;
        }
        if arg == "--" {
            after_double_dash = true;
            continue;
        }
        if arg.starts_with("--") && arg.contains('=') {
            continue;
        }
        if flags_with_vals.contains(&arg.as_str()) {
            if i + 1 < args.len() {
                skip_next = true;
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg);
    }
    out
}

fn parse_grep_like(main_cmd: &[String], args: &[String]) -> ParsedCommand {
    let args_no_connector = trim_at_connector(args);
    let mut operands = Vec::new();
    let mut pattern: Option<String> = None;
    let mut after_double_dash = false;
    let mut iter = args_no_connector.iter().peekable();
    while let Some(arg) = iter.next() {
        if after_double_dash {
            operands.push(arg);
            continue;
        }
        if arg == "--" {
            after_double_dash = true;
            continue;
        }
        match arg.as_str() {
            "-e" | "--regexp" => {
                if let Some(pat) = iter.next()
                    && pattern.is_none()
                {
                    pattern = Some(pat.clone());
                }
                continue;
            }
            "-f" | "--file" => {
                if let Some(pat_file) = iter.next()
                    && pattern.is_none()
                {
                    pattern = Some(pat_file.clone());
                }
                continue;
            }
            "-m" | "--max-count" | "-C" | "--context" | "-A" | "--after-context" | "-B"
            | "--before-context" => {
                iter.next();
                continue;
            }
            _ => {}
        }
        if arg.starts_with('-') {
            continue;
        }
        operands.push(arg);
    }
    // Do not shorten the query: grep patterns may legitimately contain slashes
    // and should be preserved verbatim. Only paths should be shortened.
    let has_pattern = pattern.is_some();
    let query = pattern.or_else(|| operands.first().cloned().map(String::from));
    let path_index = if has_pattern { 0 } else { 1 };
    let path = operands.get(path_index).map(|s| short_display_path(s));
    ParsedCommand::Search {
        cmd: shlex_join(main_cmd),
        query,
        path,
    }
}

fn awk_data_file_operand(args: &[String]) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let args_no_connector = trim_at_connector(args);
    let has_script_file = args_no_connector
        .iter()
        .any(|arg| arg == "-f" || arg == "--file");
    let candidates = skip_flag_values(
        &args_no_connector,
        &["-F", "-v", "-f", "--field-separator", "--assign", "--file"],
    );
    let non_flags: Vec<&String> = candidates
        .into_iter()
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    if has_script_file {
        return non_flags.first().cloned().cloned();
    }
    if non_flags.len() >= 2 {
        return Some(non_flags[1].clone());
    }
    None
}

fn python_walks_files(args: &[String]) -> bool {
    let args_no_connector = trim_at_connector(args);
    let mut iter = args_no_connector.iter();
    while let Some(arg) = iter.next() {
        if arg == "-c"
            && let Some(script) = iter.next()
        {
            return script.contains("os.walk")
                || script.contains("os.listdir")
                || script.contains("os.scandir")
                || script.contains("glob.glob")
                || script.contains("glob.iglob")
                || script.contains("pathlib.Path")
                || script.contains(".rglob(");
        }
    }
    false
}

fn is_python_command(cmd: &str) -> bool {
    cmd == "python"
        || cmd == "python2"
        || cmd == "python3"
        || cmd.starts_with("python2.")
        || cmd.starts_with("python3.")
}

fn cd_target(args: &[String]) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut target: Option<String> = None;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            return args.get(i + 1).cloned();
        }
        if matches!(arg.as_str(), "-L" | "-P") {
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        target = Some(arg.clone());
        i += 1;
    }
    target
}

fn is_pathish(s: &str) -> bool {
    s == "."
        || s == ".."
        || s.starts_with("./")
        || s.starts_with("../")
        || s.contains('/')
        || s.contains('\\')
}

fn parse_fd_query_and_path(tail: &[String]) -> (Option<String>, Option<String>) {
    let args_no_connector = trim_at_connector(tail);
    // fd has several flags that take values (e.g., -t/--type, -e/--extension).
    // Skip those values when extracting positional operands.
    let candidates = skip_flag_values(
        &args_no_connector,
        &[
            "-t",
            "--type",
            "-e",
            "--extension",
            "-E",
            "--exclude",
            "--search-path",
        ],
    );
    let non_flags: Vec<&String> = candidates
        .into_iter()
        .filter(|p| !p.starts_with('-'))
        .collect();
    match non_flags.as_slice() {
        [one] => {
            if is_pathish(one) {
                (None, Some(short_display_path(one)))
            } else {
                (Some((*one).clone()), None)
            }
        }
        [q, p, ..] => (Some((*q).clone()), Some(short_display_path(p))),
        _ => (None, None),
    }
}

fn parse_find_query_and_path(tail: &[String]) -> (Option<String>, Option<String>) {
    let args_no_connector = trim_at_connector(tail);
    // First positional argument (excluding common unary operators) is the root path
    let mut path: Option<String> = None;
    for a in &args_no_connector {
        if !a.starts_with('-') && *a != "!" && *a != "(" && *a != ")" {
            path = Some(short_display_path(a));
            break;
        }
    }
    // Extract a common name/path/regex pattern if present
    let mut query: Option<String> = None;
    let mut i = 0;
    while i < args_no_connector.len() {
        let a = &args_no_connector[i];
        if a == "-name" || a == "-iname" || a == "-path" || a == "-regex" {
            if i + 1 < args_no_connector.len() {
                query = Some(args_no_connector[i + 1].clone());
            }
            break;
        }
        i += 1;
    }
    (query, path)
}

fn parse_shell_lc_commands(original: &[String]) -> Option<Vec<ParsedCommand>> {
    // Only handle bash/zsh here; PowerShell is stripped separately without bash parsing.
    let (_, script) = extract_bash_command(original)?;

    if let Some(tree) = try_parse_shell(script)
        && let Some(all_commands) = try_parse_word_only_commands_sequence(&tree, script)
        && !all_commands.is_empty()
    {
        let script_tokens = shlex_split(script).unwrap_or_else(|| vec![script.to_string()]);
        // Strip small formatting helpers (e.g., head/tail/awk/wc/etc) so we
        // bias toward the primary command when pipelines are present.
        // First, drop obvious small formatting helpers (e.g., wc/awk/etc).
        let had_multiple_commands = all_commands.len() > 1;
        // Commands arrive in source order; drop formatting helpers while preserving it.
        let filtered_commands = drop_small_formatting_commands(all_commands);
        if filtered_commands.is_empty() {
            return Some(vec![ParsedCommand::Unknown {
                cmd: script.to_string(),
            }]);
        }
        // Build parsed commands, tracking `cd` segments to compute effective file paths.
        let mut commands: Vec<ParsedCommand> = Vec::new();
        let mut cwd: Option<String> = None;
        for tokens in filtered_commands.into_iter() {
            if let Some((head, tail)) = tokens.split_first()
                && head == "cd"
            {
                if let Some(dir) = cd_target(tail) {
                    cwd = Some(match &cwd {
                        Some(base) => join_paths(base, &dir),
                        None => dir.clone(),
                    });
                }
                continue;
            }
            let parsed = summarize_main_tokens(&tokens);
            let parsed = match parsed {
                ParsedCommand::Read { cmd, name, path } => {
                    if let Some(base) = &cwd {
                        let full = join_paths(base, &path.to_string_lossy());
                        ParsedCommand::Read {
                            cmd,
                            name,
                            path: PathBuf::from(full),
                        }
                    } else {
                        ParsedCommand::Read { cmd, name, path }
                    }
                }
                other => other,
            };
            commands.push(parsed);
        }

        if commands.len() > 1 {
            commands.retain(|pc| !matches!(pc, ParsedCommand::Unknown { cmd } if cmd == "true"));
            // Apply the same simplifications used for non-bash parsing, e.g., drop leading `cd`.
            while let Some(next) = simplify_once(&commands) {
                commands = next;
            }
        }
        if commands.len() == 1 {
            // If we reduced to a single command, attribute the full original script
            // for clearer UX in file-reading and listing scenarios, or when there were
            // no connectors in the original script. For pipeline commands (e.g.
            // `rg --files | sed -n`), keep only the primary command.
            let had_connectors = had_multiple_commands
                || script_tokens
                    .iter()
                    .any(|t| t == "|" || t == "&&" || t == "||" || t == ";");
            commands = commands
                .into_iter()
                .map(|pc| match pc {
                    ParsedCommand::Read { name, cmd, path } => {
                        if had_connectors {
                            let has_pipe = script_tokens.iter().any(|t| t == "|");
                            let has_sed_n = script_tokens.windows(2).any(|w| {
                                w.first().map(String::as_str) == Some("sed")
                                    && w.get(1).map(String::as_str) == Some("-n")
                            });
                            if has_pipe && has_sed_n {
                                ParsedCommand::Read {
                                    cmd: script.to_string(),
                                    name,
                                    path,
                                }
                            } else {
                                ParsedCommand::Read { cmd, name, path }
                            }
                        } else {
                            ParsedCommand::Read {
                                cmd: shlex_join(&script_tokens),
                                name,
                                path,
                            }
                        }
                    }
                    ParsedCommand::ListFiles { path, cmd, .. } => {
                        if had_connectors {
                            ParsedCommand::ListFiles { cmd, path }
                        } else {
                            ParsedCommand::ListFiles {
                                cmd: shlex_join(&script_tokens),
                                path,
                            }
                        }
                    }
                    ParsedCommand::Search {
                        query, path, cmd, ..
                    } => {
                        if had_connectors {
                            ParsedCommand::Search { cmd, query, path }
                        } else {
                            ParsedCommand::Search {
                                cmd: shlex_join(&script_tokens),
                                query,
                                path,
                            }
                        }
                    }
                    other => other,
                })
                .collect();
        }
        return Some(commands);
    }
    Some(vec![ParsedCommand::Unknown {
        cmd: script.to_string(),
    }])
}

/// Return true if this looks like a small formatting helper in a pipeline.
/// Examples: `head -n 40`, `tail -n +10`, `wc -l`, `awk ...`, `cut ...`, `tr ...`.
/// We try to keep variants that clearly include a file path (e.g. `tail -n 30 file`).
fn is_small_formatting_command(tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let cmd = tokens[0].as_str();
    match cmd {
        // Always formatting; typically used in pipes.
        // `nl` is special-cased below to allow `nl <file>` to be treated as a read command.
        "wc" | "tr" | "cut" | "sort" | "uniq" | "tee" | "column" | "yes" | "printf" => true,
        "xargs" => !is_mutating_xargs_command(tokens),
        "awk" => awk_data_file_operand(&tokens[1..]).is_none(),
        "head" => {
            // Treat as formatting when no explicit file operand is present.
            // Common forms: `head -n 40`, `head -c 100`.
            // Keep cases like `head -n 40 file`.
            match tokens {
                // `head`
                [_] => true,
                // `head <file>` or `head -n50`/`head -c100`
                [_, arg] => arg.starts_with('-'),
                // `head -n 40` / `head -c 100` (no file operand)
                [_, flag, count]
                    if (flag == "-n" || flag == "-c")
                        && count.chars().all(|c| c.is_ascii_digit()) =>
                {
                    true
                }
                _ => false,
            }
        }
        "tail" => {
            // Treat as formatting when no explicit file operand is present.
            // Common forms: `tail -n +10`, `tail -n 30`, `tail -c 100`.
            // Keep cases like `tail -n 30 file`.
            match tokens {
                // `tail`
                [_] => true,
                // `tail <file>` or `tail -n30`/`tail -n+10`
                [_, arg] => arg.starts_with('-'),
                // `tail -n 30` / `tail -n +10` (no file operand)
                [_, flag, count]
                    if flag == "-n"
                        && (count.chars().all(|c| c.is_ascii_digit())
                            || (count.starts_with('+')
                                && count[1..].chars().all(|c| c.is_ascii_digit()))) =>
                {
                    true
                }
                // `tail -c 100` / `tail -c +10` (no file operand)
                [_, flag, count]
                    if flag == "-c"
                        && (count.chars().all(|c| c.is_ascii_digit())
                            || (count.starts_with('+')
                                && count[1..].chars().all(|c| c.is_ascii_digit()))) =>
                {
                    true
                }
                _ => false,
            }
        }
        "sed" => {
            // Keep `sed -n <range> file` (treated as a file read elsewhere);
            // otherwise consider it a formatting helper in a pipeline.
            sed_read_path(&tokens[1..]).is_none()
        }
        _ => false,
    }
}

fn is_mutating_xargs_command(tokens: &[String]) -> bool {
    xargs_subcommand(tokens).is_some_and(xargs_is_mutating_subcommand)
}

fn xargs_subcommand(tokens: &[String]) -> Option<&[String]> {
    if tokens.first().map(String::as_str) != Some("xargs") {
        return None;
    }
    let mut i = 1;
    while i < tokens.len() {
        let token = &tokens[i];
        if token == "--" {
            return tokens.get(i + 1..).filter(|rest| !rest.is_empty());
        }
        if !token.starts_with('-') {
            return tokens.get(i..).filter(|rest| !rest.is_empty());
        }
        let takes_value = matches!(
            token.as_str(),
            "-E" | "-e" | "-I" | "-L" | "-n" | "-P" | "-s"
        );
        if takes_value && token.len() == 2 {
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

fn xargs_is_mutating_subcommand(tokens: &[String]) -> bool {
    let Some((head, tail)) = tokens.split_first() else {
        return false;
    };
    match head.as_str() {
        "perl" | "ruby" => xargs_has_in_place_flag(tail),
        "sed" => xargs_has_in_place_flag(tail) || tail.iter().any(|token| token == "--in-place"),
        "rg" => tail.iter().any(|token| token == "--replace"),
        _ => false,
    }
}

fn xargs_has_in_place_flag(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        token == "-i" || token.starts_with("-i") || token == "-pi" || token.starts_with("-pi")
    })
}

fn drop_small_formatting_commands(mut commands: Vec<Vec<String>>) -> Vec<Vec<String>> {
    commands.retain(|tokens| !is_small_formatting_command(tokens));
    commands
}

fn list_files_command(
    main_cmd: &[String],
    tail: &[String],
    flags_with_vals: &[&str],
) -> ParsedCommand {
    ParsedCommand::ListFiles {
        cmd: shlex_join(main_cmd),
        path: first_non_flag_operand(tail, flags_with_vals).map(|p| short_display_path(&p)),
    }
}

fn summarize_main_tokens(main_cmd: &[String]) -> ParsedCommand {
    match main_cmd.split_first() {
        Some((head, tail)) if matches!(head.as_str(), "ls" | "eza" | "exa") => {
            let flags_with_vals: &[&str] = match head.as_str() {
                "ls" => &[
                    "-I",
                    "-w",
                    "--block-size",
                    "--format",
                    "--time-style",
                    "--color",
                    "--quoting-style",
                ],
                "eza" | "exa" => &[
                    "-I",
                    "--ignore-glob",
                    "--color",
                    "--sort",
                    "--time-style",
                    "--time",
                ],
                _ => &[],
            };
            list_files_command(main_cmd, tail, flags_with_vals)
        }
        Some((head, tail)) if head == "tree" => list_files_command(
            main_cmd,
            tail,
            &["-L", "-P", "-I", "--charset", "--filelimit", "--sort"],
        ),
        Some((head, tail)) if head == "du" => list_files_command(
            main_cmd,
            tail,
            &[
                "-d",
                "--max-depth",
                "-B",
                "--block-size",
                "--exclude",
                "--time-style",
            ],
        ),
        Some((head, tail)) if head == "rg" || head == "rga" || head == "ripgrep-all" => {
            let args_no_connector = trim_at_connector(tail);
            let has_files_flag = args_no_connector.iter().any(|a| a == "--files");
            let candidates = skip_flag_values(
                &args_no_connector,
                &[
                    "-g",
                    "--glob",
                    "--iglob",
                    "-t",
                    "--type",
                    "--type-add",
                    "--type-not",
                    "-m",
                    "--max-count",
                    "-A",
                    "-B",
                    "-C",
                    "--context",
                    "--max-depth",
                ],
            );
            let non_flags: Vec<&String> = candidates
                .into_iter()
                .filter(|p| !p.starts_with('-'))
                .collect();
            if has_files_flag {
                let path = non_flags.first().map(|s| short_display_path(s));
                ParsedCommand::ListFiles {
                    cmd: shlex_join(main_cmd),
                    path,
                }
            } else {
                let query = non_flags.first().cloned().map(String::from);
                let path = non_flags.get(1).map(|s| short_display_path(s));
                ParsedCommand::Search {
                    cmd: shlex_join(main_cmd),
                    query,
                    path,
                }
            }
        }
        Some((head, tail)) if head == "git" => match tail.split_first() {
            Some((subcmd, sub_tail)) if subcmd == "grep" => parse_grep_like(main_cmd, sub_tail),
            Some((subcmd, sub_tail)) if subcmd == "ls-files" => {
                let path = first_non_flag_operand(
                    sub_tail,
                    &["--exclude", "--exclude-from", "--pathspec-from-file"],
                )
                .map(|p| short_display_path(&p));
                ParsedCommand::ListFiles {
                    cmd: shlex_join(main_cmd),
                    path,
                }
            }
            _ => ParsedCommand::Unknown {
                cmd: shlex_join(main_cmd),
            },
        },
        Some((head, tail)) if head == "fd" => {
            let (query, path) = parse_fd_query_and_path(tail);
            if query.is_some() {
                ParsedCommand::Search {
                    cmd: shlex_join(main_cmd),
                    query,
                    path,
                }
            } else {
                ParsedCommand::ListFiles {
                    cmd: shlex_join(main_cmd),
                    path,
                }
            }
        }
        Some((head, tail)) if head == "find" => {
            // Basic find support: capture path and common name filter
            let (query, path) = parse_find_query_and_path(tail);
            if query.is_some() {
                ParsedCommand::Search {
                    cmd: shlex_join(main_cmd),
                    query,
                    path,
                }
            } else {
                ParsedCommand::ListFiles {
                    cmd: shlex_join(main_cmd),
                    path,
                }
            }
        }
        Some((head, tail)) if matches!(head.as_str(), "grep" | "egrep" | "fgrep") => {
            parse_grep_like(main_cmd, tail)
        }
        Some((head, tail)) if matches!(head.as_str(), "ag" | "ack" | "pt") => {
            parse_grep_like(main_cmd, tail)
        }
        Some((head, tail)) if head == "cat" => read_from_operand(main_cmd, tail, &[]),
        Some((head, tail)) if matches!(head.as_str(), "bat" | "batcat") => read_from_operand(
            main_cmd,
            tail,
            &[
                "--theme",
                "--language",
                "--style",
                "--terminal-width",
                "--tabs",
                "--line-range",
                "--map-syntax",
            ],
        ),
        Some((head, tail)) if head == "less" => read_from_operand(
            main_cmd,
            tail,
            &[
                "-p",
                "-P",
                "-x",
                "-y",
                "-z",
                "-j",
                "--pattern",
                "--prompt",
                "--tabs",
                "--shift",
                "--jump-target",
            ],
        ),
        Some((head, tail)) if head == "more" => read_from_operand(main_cmd, tail, &[]),
        Some((head, tail)) if head == "head" => read_from_n_line_command(main_cmd, tail, false),
        Some((head, tail)) if head == "tail" => read_from_n_line_command(main_cmd, tail, true),
        Some((head, tail)) if head == "awk" => awk_data_file_operand(tail)
            .map(|path| parsed_read(main_cmd, &path))
            .unwrap_or_else(|| ParsedCommand::Unknown {
                cmd: shlex_join(main_cmd),
            }),
        Some((head, tail)) if head == "nl" => {
            skip_flag_values(tail, &["-s", "-w", "-v", "-i", "-b"])
                .into_iter()
                .find(|p| !p.starts_with('-'))
                .map(|path| parsed_read(main_cmd, path))
                .unwrap_or_else(|| ParsedCommand::Unknown {
                    cmd: shlex_join(main_cmd),
                })
        }
        Some((head, tail)) if head == "sed" => sed_read_path(tail)
            .map(|path| parsed_read(main_cmd, &path))
            .unwrap_or_else(|| ParsedCommand::Unknown {
                cmd: shlex_join(main_cmd),
            }),
        Some((head, tail)) if is_python_command(head) => {
            if python_walks_files(tail) {
                ParsedCommand::ListFiles {
                    cmd: shlex_join(main_cmd),
                    path: None,
                }
            } else {
                ParsedCommand::Unknown {
                    cmd: shlex_join(main_cmd),
                }
            }
        }
        // Other commands
        _ => ParsedCommand::Unknown {
            cmd: shlex_join(main_cmd),
        },
    }
}

fn is_abs_like(path: &str) -> bool {
    if std::path::Path::new(path).is_absolute() {
        return true;
    }
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        // Windows drive path like C:\
        (Some(d), Some(':'), Some('\\')) if d.is_ascii_alphabetic() => return true,
        // UNC path like \\server\share
        (Some('\\'), Some('\\'), _) => return true,
        _ => {}
    }
    false
}

fn join_paths(base: &str, rel: &str) -> String {
    if is_abs_like(rel) {
        return rel.to_string();
    }
    if base.is_empty() {
        return rel.to_string();
    }
    let mut buf = PathBuf::from(base);
    buf.push(rel);
    buf.to_string_lossy().to_string()
}
