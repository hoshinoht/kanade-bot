//! The chat profanity guardrail (user decision 2026-10-05): the nudge
//! deny-list over a member's question (deflected before any model call) and
//! over the finished reply (one clean retry, then the deflection line).

use serde_json::{Value, json};

use super::Generation;
use crate::chat::nudge::WordFilter;
use crate::domain::settings::Profanity;

/// Which text hit the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfanitySide {
    Question,
    Reply,
}

impl ProfanitySide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Reply => "reply",
        }
    }
}

/// One guardrail hit, logged as `guardrail.profanity`. `sent` is the line
/// sent instead; `None` when a reply's clean retry came back clean and was
/// delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfanityHit {
    pub side: ProfanitySide,
    pub word: String,
    pub sent: Option<String>,
}

impl ProfanityHit {
    pub fn to_json(&self) -> Value {
        json!({"side": self.side.as_str(), "word": self.word, "sent": self.sent})
    }
}

/// The live guardrail settings one question was prepared with.
#[derive(Clone, Debug)]
pub struct ProfanityGuard {
    words: WordFilter,
    check_questions: bool,
    check_replies: bool,
    line: String,
    /// Known names (lowercase, longest first) removed before matching.
    names: Vec<String>,
}

impl Default for ProfanityGuard {
    fn default() -> Self {
        Self::new(&Profanity::default())
    }
}

impl ProfanityGuard {
    pub fn new(settings: &Profanity) -> Self {
        Self {
            words: WordFilter::new(&settings.extra_words, &settings.allowed_words),
            check_questions: settings.check_questions,
            check_replies: settings.check_replies,
            line: settings.deflection_line.clone(),
            names: Vec::new(),
        }
    }

    /// Roster display names, nicknames and chat aliases, and the bot's own
    /// names: a real name that happens to equal a listed word never triggers.
    /// A name that is itself profanity is not exempted: one only counts when
    /// its listed words are all [`REAL_NAME_COLLISIONS`] (`Dick`, `Tai`), so a
    /// display name `shit` never unlocks `shit`.
    #[must_use]
    pub fn with_names<I: IntoIterator<Item = String>>(mut self, names: I) -> Self {
        let collisions: Vec<String> = REAL_NAME_COLLISIONS
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        let words = &self.words;
        let mut names: Vec<String> = names
            .into_iter()
            .map(|name| name.trim().to_lowercase())
            .filter(|name| name.chars().count() >= 2)
            .filter(|name| {
                words
                    .denied_strict(&without_names(name, &collisions))
                    .is_none()
            })
            .collect();
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        names.dedup();
        self.names = names;
        self
    }

    pub fn line(&self) -> &str {
        &self.line
    }

    fn hit(&self, text: &str) -> Option<String> {
        let text = without_names(&checked_text(text), &self.names);
        self.words
            .denied_strict(&text)
            .map(|hit| hit.as_str().to_owned())
    }

    /// The listed word in a member's own message, when questions are checked.
    pub fn question_hit(&self, question: &str) -> Option<String> {
        self.check_questions.then(|| self.hit(question)).flatten()
    }

    /// The listed word in the model's own reply text, when replies are
    /// checked (grounded records and card lines from the store are not the
    /// model's words and are never passed here).
    pub fn reply_hit(&self, reply: &str) -> Option<String> {
        self.check_replies.then(|| self.hit(reply)).flatten()
    }

    /// A question-side hit's whole answer: the deflection line, no model call.
    pub fn deflect(&self, word: String) -> Generation {
        Generation {
            reply: self.line.clone(),
            profanity: Some(ProfanityHit {
                side: ProfanitySide::Question,
                word,
                sent: Some(self.line.clone()),
            }),
            ..Generation::default()
        }
    }
}

/// The text the list is matched against: Discord tokens (`<@id>`, `<#id>`,
/// `<:emoji:id>`, `<t:…>`) and links are dropped first, since the matcher
/// reads digits as letters and ids or link tokens would read as words.
fn checked_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let token = &rest[start..];
        match token.find('>') {
            Some(end) if !token[1..end].contains(char::is_whitespace) => {
                out.push(' ');
                rest = &token[end + 1..];
            }
            _ => {
                out.push('<');
                rest = &token[1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace()
        .filter(|word| {
            let lower = word.to_ascii_lowercase();
            !(lower.contains("://") || lower.starts_with("www."))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Real given names and surnames that equal a deny-listed word; only these
/// may be exempted as roster names.
pub const REAL_NAME_COLLISIONS: &[&str] = &["dick", "tai", "fanny", "willy", "dik"];

/// `text` lowercased with every known name removed where it stands as its
/// own word (bounded by non-alphanumerics), so `Bob` goes but `Bobcat` stays.
fn without_names(text: &str, names: &[String]) -> String {
    let mut text = text.to_lowercase();
    for name in names {
        let mut from = 0;
        while let Some(found) = text[from..].find(name.as_str()) {
            let start = from + found;
            let end = start + name.len();
            let bounded = !text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
                && !text[end..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            if bounded {
                text.replace_range(start..end, " ");
                from = start + 1;
            } else {
                from = start + text[start..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_tokens_and_links_are_not_read_as_words() {
        let guard = ProfanityGuard::default();
        // `7a1` reads as `tai` and `8008` as `bob`/`boob` once digits map to letters.
        for clean in [
            "<@2741> <#9808> when is lotus?",
            "see https://kanade.example/e/x7a1y?t=sh1t for the run",
            "<:smug:28082> ok",
            "a < b and c > d",
        ] {
            assert_eq!(guard.question_hit(clean), None, "{clean:?}");
        }
        assert_eq!(
            guard.question_hit("<@1> this is sh1t").as_deref(),
            Some("shit")
        );
    }

    /// Common names never trigger: collapsing letters cannot make `boob`
    /// from `Bob`, and short entries match only exactly. A real name equal
    /// to a listed word passes once it is on the roster.
    #[test]
    fn member_names_do_not_trigger() {
        let guard = ProfanityGuard::default();
        for clean in [
            "Bob",
            "Bobby",
            "Bobb",
            "bob's run",
            "Taiga",
            "Taiyo",
            "Dickens",
        ] {
            assert_eq!(guard.reply_hit(clean), None, "{clean}");
        }
        assert_eq!(guard.reply_hit("boob").as_deref(), Some("boob"));
        assert_eq!(guard.reply_hit("booobs").as_deref(), Some("boob"));
        assert_eq!(guard.reply_hit("Tai is in").as_deref(), Some("tai"));
        assert_eq!(guard.reply_hit("tais"), None, "tai is exact-only");
        for clean in [
            "cumin",
            "Hoey",
            "hoed",
            "cocky",
            "titter",
            "asus",
            "Essex",
            "Sussex",
            "analysis",
            "Scunthorpe",
            "shift",
            "begin",
            "class",
            "assess",
            "pass",
            "Dickens",
        ] {
            assert_eq!(guard.reply_hit(clean), None, "{clean}");
        }
        let roster = ProfanityGuard::default().with_names(
            [
                "Tai",
                "Dick",
                "Dick Tan",
                "Big Dick Energy",
                "shit",
                "fuckboi",
                "x",
            ]
            .map(str::to_owned),
        );
        for clean in [
            "Tai and Dick are in Lotus.",
            "<@1> Dick Tan, when is lotus?",
        ] {
            assert_eq!(roster.question_hit(clean), None, "{clean}");
        }
        assert_eq!(
            roster.reply_hit("Dick, that's shit").as_deref(),
            Some("shit")
        );
        assert_eq!(
            roster.reply_hit("shit").as_deref(),
            Some("shit"),
            "not unlocked"
        );
        assert_eq!(roster.reply_hit("fuckboi").as_deref(), Some("fuck"));
        // Judged word by word: its only listed word is a real-name collision.
        assert_eq!(roster.reply_hit("Big Dick Energy"), None);
        assert_eq!(guard.reply_hit("Big Dick Energy").as_deref(), Some("dick"));
    }

    /// Recall: the variants members type still hit the chat matcher.
    #[test]
    fn typed_variants_still_hit() {
        let guard = ProfanityGuard::default();
        for (typed, entry) in [
            ("fking", "fk"),
            ("fcking", "fck"),
            ("fuking", "fuk"),
            ("fukin", "fuk"),
            ("tits", "tit"),
            ("titty", "tit"),
            ("sexy", "sex"),
            ("hoes", "hoe"),
            ("cums", "cum"),
            ("fags", "fag"),
            ("wtfff", "wtf"),
            ("fuuuk", "fuk"),
            ("ashole", "asshole"),
            ("a$hole", "asshole"),
            ("pusy", "pussy"),
            ("jiz", "jizz"),
            ("chebai", "cheebai"),
            ("diuuu", "diu"),
            ("boob", "boob"),
            ("booobies", "boob"),
            ("tai", "tai"),
            ("asu", "asu"),
        ] {
            assert_eq!(guard.reply_hit(typed).as_deref(), Some(entry), "{typed}");
        }
    }

    /// Meso amounts and counts are numbers, not leetspeak; digits inside a
    /// word still map.
    #[test]
    fn numbers_are_not_leetspeak() {
        let guard = ProfanityGuard::default();
        for clean in [
            "selling for 800b",
            "8008 mesos",
            "717 runs",
            "1.5b, 800B.",
            "7175k",
            "run 3 at 21:00",
        ] {
            assert_eq!(guard.reply_hit(clean), None, "{clean}");
            assert_eq!(guard.question_hit(clean), None, "{clean}");
        }
        assert_eq!(guard.reply_hit("5h1t").as_deref(), Some("shit"));
        assert_eq!(guard.reply_hit("b00b").as_deref(), Some("boob"));
    }

    /// A regression pin, not an exhaustive check: built-in entries' surface
    /// forms (itself, its collapse, each suffix) against the known common
    /// words and names they collide with; none may hit.
    #[test]
    fn known_common_forms_stay_clean() {
        use crate::chat::nudge::{EXACT_ONLY, RUN_STRICT, builtin_words};
        const COMMON: &[&str] = &[
            "bob", "bobs", "bobby", "bobbie", "bobbing", "bobbed", "tais", "asus", "cumin", "hoey",
            "hoed", "cocky", "cocker", "cocking", "titer", "titter", "kuya", "yedi", "dikes",
            "babies",
        ];
        let guard = ProfanityGuard::default();
        let collapse = |word: &str| {
            let mut out = String::new();
            for c in word.chars() {
                if !out.ends_with(c) {
                    out.push(c);
                }
            }
            out
        };
        let mut collisions = Vec::new();
        for entry in builtin_words() {
            let mut forms = vec![entry.to_owned(), collapse(entry)];
            for suffix in ["s", "es", "ed", "er", "ing", "in", "y", "ty", "ies"] {
                forms.push(format!("{entry}{suffix}"));
                forms.push(format!("{}{suffix}", collapse(entry)));
            }
            for form in forms {
                if COMMON.contains(&form.as_str())
                    && !EXACT_ONLY.contains(&form.as_str())
                    && guard.reply_hit(&form).is_some()
                {
                    collisions.push((entry, form));
                }
            }
        }
        assert!(collisions.is_empty(), "{collisions:?}");
        assert_eq!(collapse("boob"), "bob");
        assert_eq!(collapse("nigger"), "niger");
        assert!(RUN_STRICT.contains(&"boob") && RUN_STRICT.contains(&"nigger"));
    }

    #[test]
    fn switches_turn_each_side_off() {
        let off = ProfanityGuard::new(&Profanity {
            check_questions: false,
            check_replies: false,
            ..Profanity::default()
        });
        assert_eq!(off.question_hit("fuck"), None);
        assert_eq!(off.reply_hit("fuck"), None);
        let on = ProfanityGuard::default();
        assert_eq!(on.reply_hit("well fuck").as_deref(), Some("fuck"));
    }
}
