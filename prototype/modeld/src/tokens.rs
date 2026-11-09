//! Rough token estimate for the context budget.
//!
//! The prototype has no tokenizer (that arrives with the in-kernel engine), so
//! the budget is computed from a documented heuristic. It is deliberately
//! pessimistic for Cyrillic, where one token is usually shorter than four
//! characters: we count `ceil(chars / 4)` and never exceed the budget by much.
//!
//! Exact usage is not estimated at all: ollama reports `prompt_eval_count` and
//! `eval_count` in the final stream chunk, and the console stores those numbers
//! in the session log (see [`crate::GenStats`]).

/// Estimates the token count of `text`.
///
/// * empty text → `0`;
/// * otherwise `ceil(chars / 4)`, at least `1`.
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        return 0;
    }
    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::estimate_tokens;

    #[test]
    fn empty_text_is_zero_tokens() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_is_roughly_four_chars_per_token() {
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // Cyrillic: one char is two UTF-8 bytes, still one "character".
        assert_eq!(estimate_tokens("привет"), 2);
        assert_eq!(estimate_tokens(&"x".repeat(4000)), 1000);
    }

    #[test]
    fn estimate_is_monotonic() {
        let mut previous = 0;
        for len in 0..200 {
            let current = estimate_tokens(&"y".repeat(len));
            assert!(current >= previous, "len={len}");
            previous = current;
        }
    }
}
