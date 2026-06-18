//! Fuzzy matching helpers for UI filtering and highlighting.
//!
//! The matcher returns original character indices so callers can highlight the
//! source string without translating from normalized search text.

/// Simple case-insensitive subsequence matcher used for fuzzy filtering.
///
/// Returns the indices (character positions) of the matched characters in the
/// ORIGINAL `haystack` string and a score where smaller is better.
///
/// Unicode correctness: we match against a lowercased haystack and stream the
/// lowercased needle, while maintaining a mapping from each character in the
/// lowercased haystack back to the original character index in `haystack`. This
/// ensures the returned indices can be safely used with `str::chars().enumerate()`
/// consumers for highlighting, even when lowercasing expands certain characters
/// (e.g., ß → ss, İ → i̇).
pub fn fuzzy_match(haystack: &str, needle: &str) -> Option<(Vec<usize>, i32)> {
    if needle.is_empty() {
        return Some((Vec::new(), i32::MAX));
    }

    if haystack.is_ascii() && needle.is_ascii() {
        let haystack_bytes = haystack.as_bytes();
        let mut result_indices = Vec::with_capacity(needle.len());
        let mut cur = 0usize;

        for needle_byte in needle.bytes() {
            let mut found_at = None;
            while cur < haystack_bytes.len() {
                if haystack_bytes[cur].eq_ignore_ascii_case(&needle_byte) {
                    found_at = Some(cur);
                    cur += 1;
                    break;
                }
                cur += 1;
            }
            let pos = found_at?;
            result_indices.push(pos);
        }

        let first_pos = result_indices[0];
        let last_pos = result_indices[result_indices.len() - 1];
        return Some((
            result_indices,
            score_match_window(first_pos, last_pos, needle.len()),
        ));
    }

    let mut result_orig_indices: Vec<usize> = Vec::with_capacity(needle.chars().count());
    let mut lowered_needle = needle.chars().flat_map(char::to_lowercase);
    let Some(mut target_char) = lowered_needle.next() else {
        return Some((Vec::new(), i32::MAX));
    };
    let mut lowered_needle_len = 0usize;
    let mut first_lower_pos: Option<usize> = None;
    let mut lower_pos = 0usize;

    // Stream the lowercased haystack instead of storing a normalized copy plus
    // an index map. The first lowered position for a matched original character
    // is retained so multi-codepoint expansions keep the documented scoring.
    for (orig_idx, ch) in haystack.chars().enumerate() {
        let orig_first_lower_pos = lower_pos;
        for haystack_char in ch.to_lowercase() {
            if haystack_char == target_char {
                // A single source character can lowercase into multiple
                // characters. Keep highlight indices unique as we stream so the
                // successful path does not need a sort/dedup pass later.
                if result_orig_indices.last().copied() != Some(orig_idx) {
                    result_orig_indices.push(orig_idx);
                }
                first_lower_pos.get_or_insert(orig_first_lower_pos);

                if let Some(next_char) = lowered_needle.next() {
                    target_char = next_char;
                    lowered_needle_len += 1;
                } else {
                    let first_lower_pos = first_lower_pos.unwrap_or(0);
                    let score =
                        score_match_window(first_lower_pos, lower_pos, lowered_needle_len + 1);

                    return Some((result_orig_indices, score));
                }
            }
            lower_pos += 1;
        }
    }

    None
}

fn score_match_window(first_pos: usize, last_pos: usize, needle_len: usize) -> i32 {
    let window = (last_pos as i32 - first_pos as i32 + 1) - needle_len as i32;
    let mut score = window.max(0);
    if first_pos == 0 {
        score -= 100;
    }
    score
}

/// Convenience wrapper to get only the indices for a fuzzy match.
pub fn fuzzy_indices(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    fuzzy_match(haystack, needle).map(|(idx, _)| idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn ascii_basic_indices() {
        let (idx, score) = match fuzzy_match("hello", "hl") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 2]);
        // 'h' at 0, 'l' at 2 -> window 1; start-of-string bonus applies (-100)
        assert_eq!(score, -99);
    }

    #[test]
    fn unicode_dotted_i_istanbul_highlighting() {
        let (idx, score) = match fuzzy_match("İstanbul", "is") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 1]);
        // Matches at lowered positions 0 and 2 -> window 1; start-of-string bonus applies
        assert_eq!(score, -99);
    }

    #[test]
    fn unicode_german_sharp_s_casefold() {
        assert!(fuzzy_match("straße", "strasse").is_none());
    }

    #[test]
    fn prefer_contiguous_match_over_spread() {
        let (_idx_a, score_a) = match fuzzy_match("abc", "abc") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        let (_idx_b, score_b) = match fuzzy_match("a-b-c", "abc") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        // Contiguous window -> 0; start-of-string bonus -> -100
        assert_eq!(score_a, -100);
        // Spread over 5 chars for 3-letter needle -> window 2; with bonus -> -98
        assert_eq!(score_b, -98);
        assert!(score_a < score_b);
    }

    #[test]
    fn start_of_string_bonus_applies() {
        let (_idx_a, score_a) = match fuzzy_match("file_name", "file") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        let (_idx_b, score_b) = match fuzzy_match("my_file_name", "file") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        // Start-of-string contiguous -> window 0; bonus -> -100
        assert_eq!(score_a, -100);
        // Non-prefix contiguous -> window 0; no bonus -> 0
        assert_eq!(score_b, 0);
        assert!(score_a < score_b);
    }

    #[test]
    fn empty_needle_matches_with_max_score_and_no_indices() {
        let (idx, score) = match fuzzy_match("anything", "") {
            Some(v) => v,
            None => panic!("empty needle should match"),
        };
        assert!(idx.is_empty());
        assert_eq!(score, i32::MAX);
    }

    #[test]
    fn case_insensitive_matching_basic() {
        let (idx, score) = match fuzzy_match("FooBar", "foO") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 1, 2]);
        // Contiguous prefix match (case-insensitive) -> window 0 with bonus
        assert_eq!(score, -100);
    }

    #[test]
    fn ascii_fast_path_preserves_gap_scoring() {
        let (idx, score) = match fuzzy_match("a_B_c", "abc") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 2, 4]);
        assert_eq!(score, -98);
    }

    #[test]
    fn indices_are_deduped_for_multichar_lowercase_expansion() {
        let needle = "\u{0069}\u{0307}"; // "i" + combining dot above
        let (idx, score) = match fuzzy_match("İ", needle) {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0]);
        // Lowercasing 'İ' expands to two chars; contiguous prefix -> window 0 with bonus
        assert_eq!(score, -100);
    }
}
