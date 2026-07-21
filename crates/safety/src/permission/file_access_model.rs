//! Infer file operands and access modes from one shell command invocation.

use std::path::Path;

use super::bash_command_splitting::basename;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileMode {
    Read,
    Grep {
        glob: Option<String>,
        recursive: bool,
    },
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExtractedAccess {
    Path { mode: FileMode, path: String },
    Ambiguous,
}

pub(super) fn command_file_accesses(words: &[String]) -> Vec<ExtractedAccess> {
    let Some(program) = words
        .first()
        .map(|word| basename(word).to_ascii_lowercase())
    else {
        return Vec::new();
    };
    if program == "dd" {
        return words
            .iter()
            .skip(1)
            .filter_map(|word| {
                word.strip_prefix("if=")
                    .and_then(|path| extracted_path(FileMode::Read, path.to_string()))
                    .or_else(|| {
                        word.strip_prefix("of=")
                            .and_then(|path| extracted_path(FileMode::Edit, path.to_string()))
                    })
            })
            .collect();
    }

    match program.as_str() {
        "cp" | "mv" | "ln" | "install" => path_command_accesses(&program, words),
        "rm" | "rmdir" | "mkdir" | "touch" => file_candidates(words)
            .into_iter()
            .filter_map(|path| extracted_path(FileMode::Edit, path.to_string()))
            .collect(),
        "tee" | "truncate" | "set-content" | "out-file" | "add-content" | "tee-object" => {
            file_candidates(words)
                .into_iter()
                .filter_map(|path| extracted_path(FileMode::Edit, path.to_string()))
                .collect()
        }
        "sed" if sed_is_in_place(words) => file_candidates(words)
            .into_iter()
            .skip(1)
            .flat_map(|path| {
                [FileMode::Read, FileMode::Edit]
                    .into_iter()
                    .filter_map(move |mode| extracted_path(mode, path.to_string()))
            })
            .collect(),
        "sort" => sort_accesses(words),
        "go" | "git" | "rustc" => {
            let accesses = output_paths(&program, words);
            if accesses.is_empty() {
                vec![ExtractedAccess::Ambiguous]
            } else {
                accesses
            }
        }
        program if is_reader(program) => reader_accesses(program, words),
        "rustfmt" => file_candidates(words)
            .into_iter()
            .filter_map(|path| extracted_path(FileMode::Edit, path.to_string()))
            .collect(),
        program if is_modeled_no_file(program) => Vec::new(),
        _ => vec![ExtractedAccess::Ambiguous],
    }
}

fn path_command_accesses(program: &str, words: &[String]) -> Vec<ExtractedAccess> {
    const TARGET_FLAGS: &[&str] = &["-t", "--target-directory"];
    let targets = value_flag_values(words, TARGET_FLAGS);
    if targets.is_empty() {
        let candidates = file_candidates(words);
        let Some((destination, sources)) = candidates.split_last() else {
            return Vec::new();
        };
        let mut accesses = Vec::new();
        for source in sources {
            append_source_accesses(&mut accesses, program, source);
        }
        accesses.extend(extracted_path(FileMode::Edit, (*destination).to_string()));
        return accesses;
    }
    let [target] = targets.as_slice() else {
        return vec![ExtractedAccess::Ambiguous];
    };
    let sources = file_candidates_without_value_flags(words, TARGET_FLAGS);
    if sources.is_empty() {
        return vec![ExtractedAccess::Ambiguous];
    }
    let mut accesses = Vec::new();
    for source in sources {
        append_source_accesses(&mut accesses, program, source);
        let Some(file_name) = Path::new(source).file_name() else {
            accesses.push(ExtractedAccess::Ambiguous);
            continue;
        };
        let destination = Path::new(target)
            .join(file_name)
            .to_string_lossy()
            .into_owned();
        if let Some(access) = extracted_path(FileMode::Edit, destination) {
            accesses.push(access);
        }
    }
    accesses
}

fn append_source_accesses(accesses: &mut Vec<ExtractedAccess>, program: &str, source: &str) {
    accesses.extend(extracted_path(FileMode::Read, source.to_string()));
    if program == "mv" {
        accesses.extend(extracted_path(FileMode::Edit, source.to_string()));
    }
}

fn reader_accesses(program: &str, words: &[String]) -> Vec<ExtractedAccess> {
    const PATTERN_FLAGS: &[&str] = &["-e", "--regexp", "-f", "--file"];
    let search = matches!(program, "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack");
    if !search {
        return file_candidates(words)
            .into_iter()
            .filter_map(|path| extracted_path(FileMode::Read, path.to_string()))
            .collect();
    }
    let patterns_from_flags = value_flag_values(words, PATTERN_FLAGS);
    let pattern_files = value_flag_values(words, &["-f", "--file"]);
    let candidates = file_candidates_without_value_flags(words, PATTERN_FLAGS);
    let paths = if patterns_from_flags.is_empty() {
        candidates.get(1..).unwrap_or_default()
    } else {
        candidates.as_slice()
    };
    let recursive = matches!(program, "rg" | "ag" | "ack")
        || (matches!(program, "grep" | "egrep" | "fgrep") && grep_is_recursive(words));
    let mut accesses = pattern_files
        .into_iter()
        .filter_map(|path| extracted_path(FileMode::Read, path.to_string()))
        .chain(paths.iter().filter_map(|path| {
            extracted_path(
                FileMode::Grep {
                    glob: None,
                    recursive,
                },
                (*path).to_string(),
            )
        }))
        .collect::<Vec<_>>();
    if recursive && paths.is_empty() {
        accesses.extend(extracted_path(
            FileMode::Grep {
                glob: None,
                recursive: true,
            },
            ".".to_string(),
        ));
    }
    accesses
}

fn grep_is_recursive(words: &[String]) -> bool {
    words.iter().skip(1).any(|word| {
        matches!(word.as_str(), "-r" | "-R" | "--recursive")
            || word
                .strip_prefix('-')
                .filter(|options| !options.starts_with('-'))
                .is_some_and(|options| options.contains(['r', 'R']))
    })
}

pub(super) fn extracted_path(mode: FileMode, path: String) -> Option<ExtractedAccess> {
    if path == "-" || (matches!(mode, FileMode::Edit) && is_safe_write_sink(&path)) {
        None
    } else if path.contains(['*', '?', '[']) || path.starts_with("~/") {
        Some(ExtractedAccess::Ambiguous)
    } else {
        Some(ExtractedAccess::Path { mode, path })
    }
}

fn file_candidates(words: &[String]) -> Vec<&str> {
    file_candidates_without_value_flags(words, &[])
}

fn file_candidates_without_value_flags<'a>(
    words: &'a [String],
    value_flags: &[&str],
) -> Vec<&'a str> {
    let mut candidates = Vec::new();
    let mut options_ended = false;
    let mut skip_value = false;
    for word in words.iter().skip(1) {
        if skip_value {
            skip_value = false;
            continue;
        }
        if !options_ended && word == "--" {
            options_ended = true;
        } else if !options_ended && value_flags.contains(&word.as_str()) {
            skip_value = true;
        } else if !options_ended
            && value_flags
                .iter()
                .any(|flag| flag_value(word, flag).is_some())
        {
            continue;
        } else if options_ended || (word != "-" && !word.starts_with('-')) {
            candidates.push(word.as_str());
        }
    }
    candidates
}

fn value_flag_values<'a>(words: &'a [String], flags: &[&str]) -> Vec<&'a str> {
    words
        .iter()
        .enumerate()
        .flat_map(|(index, word)| {
            flags.iter().filter_map(move |flag| {
                flag_value(word, flag).or_else(|| {
                    (word == flag)
                        .then(|| words.get(index + 1).map(String::as_str))
                        .flatten()
                })
            })
        })
        .collect()
}

fn flag_value<'a>(word: &'a str, flag: &str) -> Option<&'a str> {
    word.strip_prefix(&format!("{flag}="))
        .or_else(|| {
            (!flag.starts_with("--"))
                .then(|| word.strip_prefix(flag))
                .flatten()
        })
        .filter(|value| !value.is_empty())
}

fn output_paths(program: &str, words: &[String]) -> Vec<ExtractedAccess> {
    let mut paths = value_flag_values(words, &["--output", "-o"])
        .into_iter()
        .filter_map(|path| extracted_path(FileMode::Edit, path.to_string()))
        .collect::<Vec<_>>();
    if program == "rustc" {
        let out_dirs = value_flag_values(words, &["--out-dir"]);
        paths.extend(
            out_dirs
                .iter()
                .filter_map(|path| extracted_path(FileMode::Edit, (*path).to_string())),
        );
        if !out_dirs.is_empty() {
            paths.push(ExtractedAccess::Ambiguous);
        }
    }
    paths
}

fn sort_accesses(words: &[String]) -> Vec<ExtractedAccess> {
    const OUTPUT_FLAGS: &[&str] = &["--output", "-o"];
    file_candidates_without_value_flags(words, OUTPUT_FLAGS)
        .into_iter()
        .filter_map(|path| extracted_path(FileMode::Read, path.to_string()))
        .chain(
            value_flag_values(words, OUTPUT_FLAGS)
                .into_iter()
                .filter_map(|path| extracted_path(FileMode::Edit, path.to_string())),
        )
        .collect()
}

fn sed_is_in_place(words: &[String]) -> bool {
    words.iter().skip(1).any(|word| {
        word == "--in-place"
            || word.starts_with("--in-place=")
            || (word.starts_with('-') && !word.starts_with("--") && word.contains('i'))
    })
}

fn is_reader(program: &str) -> bool {
    matches!(
        program,
        "cat"
            | "tac"
            | "nl"
            | "head"
            | "tail"
            | "grep"
            | "egrep"
            | "fgrep"
            | "rg"
            | "sed"
            | "awk"
            | "less"
            | "more"
            | "bat"
            | "strings"
            | "xxd"
            | "od"
            | "hexdump"
            | "base64"
            | "base32"
            | "cut"
            | "sort"
            | "uniq"
            | "wc"
            | "diff"
            | "comm"
            | "jq"
            | "yq"
            | "ag"
            | "ack"
    )
}

fn is_modeled_no_file(program: &str) -> bool {
    matches!(
        program,
        ":" | "["
            | "alias"
            | "cd"
            | "echo"
            | "exit"
            | "export"
            | "false"
            | "printf"
            | "pwd"
            | "test"
            | "true"
            | "type"
            | "unalias"
            | "unset"
    )
}

fn is_safe_write_sink(path: &str) -> bool {
    matches!(path, "/dev/null" | "/dev/stdout" | "/dev/stderr")
}
