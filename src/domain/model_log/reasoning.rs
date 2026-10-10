//! Response-only diagnostics retained with their model-log row.

pub const REASONING_CAP: usize = 64 * 1024;
pub const REASONING_TRUNCATED: &str = "\n… [reasoning truncated]";

/// Includes the visible marker in the byte budget; never splits UTF-8.
pub fn capped_reasoning(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    if text.len() <= REASONING_CAP {
        return Some(text.to_owned());
    }
    let mut end = REASONING_CAP - REASONING_TRUNCATED.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}{REASONING_TRUNCATED}", &text[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_cap_includes_marker_and_preserves_utf8() {
        assert_eq!(capped_reasoning(""), None);
        assert_eq!(capped_reasoning("summary"), Some("summary".into()));
        let text = "奏".repeat(REASONING_CAP);
        let capped = capped_reasoning(&text).expect("text");
        assert!(capped.len() <= REASONING_CAP);
        assert!(capped.ends_with(REASONING_TRUNCATED));
        assert_eq!(capped_reasoning(&capped).as_deref(), Some(capped.as_str()));
    }
}
