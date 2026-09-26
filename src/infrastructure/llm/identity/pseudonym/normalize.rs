//! Name normalisation and near-match rules for pool filtering (not used for
//! matching text, which is exact up to Unicode lowercase).

/// Lowercase, keep only letters and digits, fold macron/circumflex vowels and
/// long vowels (`ou`, `oo` → `o`, `uu` → `u`, any doubled vowel → one).
pub(super) fn normalise(name: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(name.len());
    for ch in name.chars().flat_map(char::to_lowercase) {
        let ch = match ch {
            'ā' | 'â' => 'a',
            'ī' | 'î' => 'i',
            'ū' | 'û' => 'u',
            'ē' | 'ê' => 'e',
            'ō' | 'ô' => 'o',
            other => other,
        };
        if !ch.is_alphanumeric() {
            continue;
        }
        let long = matches!(
            (out.last(), ch),
            (Some('o'), 'u' | 'o')
                | (Some('u'), 'u')
                | (Some('a'), 'a')
                | (Some('i'), 'i')
                | (Some('e'), 'e')
        );
        if !long {
            out.push(ch);
        }
    }
    out.into_iter().collect()
}

/// Normalised forms a name blocks: the whole name and each of its words.
pub(super) fn name_keys(name: &str) -> Vec<String> {
    let mut keys = vec![normalise(name)];
    for word in name.split(|c: char| !c.is_alphanumeric()) {
        let key = normalise(word);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.retain(|key| !key.is_empty());
    keys
}

/// A pool name (normalised) is too close to a blocked key when they are equal,
/// one contains the other (the contained side at least 3 characters), or both
/// are at least 3 characters and one edit apart.
pub(super) fn near_match(pool: &str, key: &str) -> bool {
    if pool == key {
        return true;
    }
    let (p, k) = (pool.chars().count(), key.chars().count());
    (k >= 3 && pool.contains(key))
        || (p >= 3 && key.contains(pool))
        || (p >= 3 && k >= 3 && within_one_edit(pool, key))
}

fn within_one_edit(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if long.len() - short.len() > 1 {
        return false;
    }
    let prefix = short
        .iter()
        .zip(long.iter())
        .take_while(|(x, y)| x == y)
        .count();
    if prefix == short.len() {
        true
    } else if short.len() == long.len() {
        short[prefix + 1..] == long[prefix + 1..]
    } else {
        short[prefix..] == long[prefix + 1..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_punctuation_and_long_vowels() {
        assert_eq!(normalise("Yuuki"), "yuki");
        assert_eq!(normalise("Kyōko"), "kyoko");
        assert_eq!(normalise("KYOUKO!!"), "kyoko");
        assert_eq!(normalise("~Sou-ta_99~"), "sota99");
        assert_eq!(normalise("李小龍"), "李小龍");
    }

    #[test]
    fn near_match_rules() {
        assert!(near_match("haruka", "haruka"));
        assert!(near_match("haruka", "xharukax"));
        assert!(near_match("sayaka", "aya"));
        assert!(!near_match("sayaka", "ay"));
        assert!(near_match("yuki", "yuka"));
        assert!(near_match("yuki", "yuk"));
        assert!(near_match("yuki", "ayuki"));
        assert!(!near_match("yuki", "yoko"));
        assert!(!near_match("kaito", "kain"));
        assert!(within_one_edit("abc", "abd"));
        assert!(within_one_edit("abc", "abc"));
        assert!(!within_one_edit("abc", "bca"));
    }
}
