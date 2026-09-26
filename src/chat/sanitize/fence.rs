//! Fenced code blocks pass through reply shaping byte for byte (named v5
//! difference `D-CODE-FENCES`: v4 collapsed their indentation).

const FENCE: &str = "```";

/// Apply `prose` to the text outside paired ``` fences; each fenced block,
/// fences included, is kept verbatim. An unpaired fence is prose, as Discord
/// renders it literally.
pub(super) fn map_prose(text: &str, prose: impl Fn(&str) -> String) -> String {
    let fences: Vec<usize> = text.match_indices(FENCE).map(|(at, _)| at).collect();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for &[open, close] in fences.as_chunks::<2>().0 {
        let close = close + FENCE.len();
        out.push_str(&prose(&text[last..open]));
        out.push_str(&text[open..close]);
        last = close;
    }
    out.push_str(&prose(&text[last..]));
    out
}

#[cfg(test)]
mod tests {
    use super::map_prose;

    fn upper(text: &str) -> String {
        text.to_uppercase()
    }

    #[test]
    fn only_prose_outside_paired_fences_is_mapped() {
        assert_eq!(
            map_prose("a ```py\n  x``` b ```c``` d", upper),
            "A ```py\n  x``` B ```c``` D"
        );
    }

    #[test]
    fn an_unpaired_fence_is_prose() {
        assert_eq!(map_prose("a ```x``` b ```y", upper), "A ```x``` B ```Y");
        assert_eq!(map_prose("plain", upper), "PLAIN");
        assert_eq!(map_prose("", upper), "");
    }
}
