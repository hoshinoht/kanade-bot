//! Extra checks for model-written lead-ins (seed lines are human-approved and
//! skip them): no Discord markdown, no invisible format characters, no invite
//! links, and a small code-owned SFW deny-list (user decision 2026-09-25).
//! The chat profanity guardrail reuses the deny-list as a [`WordFilter`]
//! (built-ins minus admin-allowed words plus admin extras, 2026-10-05).

use std::sync::LazyLock;

/// Characters Discord renders as formatting. A single `~` is plain text (a
/// persona's "on you~"); only `~~` strikes through.
const MARKUP_CHARS: [char; 5] = ['`', '*', '_', '|', '\\'];
const MARKUP_PAIRS: [&str; 1] = ["~~"];

const INVITES: [&str; 3] = ["discord.gg/", "discord.com/invite", "discordapp.com/invite"];

/// Whole words (after normalisation), each also matched with a suffix from
/// [`SUFFIXES`]. Kept readable: false positives only cost the seed line.
/// `ass` is absent on purpose: letter-collapsing makes it `as`.
pub const DENY_LIST: &[&str] = &[
    "anal", "arse", "asshole", "bastard", "bitch", "blowjob", "boob", "cock", "cum", "cunt",
    "dick", "dildo", "erotic", "fap", "faggot", "fetish", "fuck", "hentai", "horny", "kinky",
    "lewd", "milf", "naked", "nigger", "nsfw", "nude", "orgasm", "penis", "porn", "pussy", "rape",
    "retard", "sex", "shit", "slut",
];

/// Sound-alike and clipped spellings (user request 2026-09-25), whole-word.
pub const DENY_SOUNDALIKE: &[&str] = &[
    "bih", "biatch", "biotch", "boner", "cawk", "cooch", "coochie", "dih", "dik", "diq", "fag",
    "fck", "fcuk", "fk", "fuk", "fuq", "fvck", "hoe", "jizz", "kys", "nigga", "nibba", "phuck",
    "phuk", "phuq", "prick", "secks", "segs", "seggs", "shyt", "stfu", "thot", "tit", "twat",
    "wank", "wtf",
];

/// Southeast Asian swears and slurs, romanised as typed in chat (user request
/// 2026-09-25): Malay/Indonesian, Singlish/Hokkien/Cantonese, Tagalog, Thai
/// and Vietnamese. Whole-word; ambiguous short forms (`dm`, `cb`, `knn`, bare
/// Vietnamese without diacritics) are left out because they collide with
/// everyday words once collapsed.
pub const DENY_SEA: &[&str] = &[
    // Malay / Indonesian / Javanese
    "anjing",
    "asu",
    "babi",
    "bajingan",
    "bangsat",
    "bodoh",
    "brengsek",
    "burit",
    "butoh",
    "celaka",
    "entot",
    "goblok",
    "jadah",
    "jancok",
    "jancuk",
    "jubur",
    "kampang",
    "keparat",
    "kimak",
    "konek",
    "kontol",
    "lahanat",
    "memek",
    "ngentot",
    "pantat",
    "pepek",
    "puki",
    "pukimak",
    "sial",
    "sundal",
    "tahi",
    "tai",
    "tolol",
    // Singlish / Hokkien / Cantonese
    "cheebai",
    "chibai",
    "cibai",
    "diu",
    "jibai",
    "kanasai",
    "kanina",
    "kaninabu",
    "lanjiao",
    "lanjiau",
    "lancau",
    "nabei",
    "pundek",
    "pundeh",
    "sohai",
    "sorhai",
    // Tagalog
    "bilat",
    "burat",
    "gago",
    "hindot",
    "jakol",
    "kantot",
    "kupal",
    "pakshet",
    "pakshit",
    "pakyu",
    "pekpek",
    "punyeta",
    "puta",
    "putangina",
    "tangina",
    "tarantado",
    "tite",
    "ulol",
    // Thai
    "kuay",
    "kuy",
    "yed",
    // Vietnamese
    "cailon",
    "cặc",
    "clgt",
    "ditme",
    "dume",
    "đéo",
    "đĩ",
    "địt",
    "đmm",
    "đụ",
    "lồn",
    "vcl",
    "vkl",
];

const SUFFIXES: [&str; 9] = ["s", "es", "ed", "er", "ing", "in", "y", "ty", "ies"];

/// Matched anywhere inside a normalised word (`bullshit`, `motherfucker`,
/// `pukimakkau`). Entries are already letter-collapsed, so `niger`/`fagot`/
/// `niga` also cover the double-g spellings. `cunt` stays whole-word only: as a
/// substring it would reject "Scunthorpe".
pub const DENY_INSIDE: &[&str] = &[
    "fuck",
    "shit",
    "niger",
    "niga",
    "fagot",
    "kontol",
    "pukimak",
    "ngentot",
    "putangina",
    "tangina",
    "cibai",
    "chibai",
    "lanjiao",
];

/// Markdown syntax at the start of the line or anywhere inline.
pub fn has_markup(line: &str) -> bool {
    let start = line.trim_start();
    let block = ["#", "-#", ">", "- ", "* ", "+ "]
        .iter()
        .any(|prefix| start.starts_with(prefix));
    let digits = start.chars().take_while(char::is_ascii_digit).count();
    let numbered = digits > 0 && start[digits..].starts_with(". ");
    block
        || numbered
        || line.contains(MARKUP_CHARS)
        || MARKUP_PAIRS.iter().any(|pair| line.contains(pair))
}

/// Unicode general category Cf (bidi controls, zero-width characters, tags...).
pub fn has_format_char(line: &str) -> bool {
    line.chars().any(|c| {
        matches!(u32::from(c),
            0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
            | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
            | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
    })
}

pub fn has_invite(line: &str) -> bool {
    let lower = line.to_lowercase();
    INVITES.iter().any(|invite| lower.contains(invite))
}

/// Leetspeak digits/symbols become letters, then runs of one letter collapse
/// (`sh1iiit` → `shit`); words split on anything that is not a letter.
fn normalise(text: &str) -> String {
    leet(text, true)
}

/// [`normalise`]'s leetspeak mapping, with or without collapsing runs.
fn leet(text: &str, collapse_runs: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    for (at, &c) in chars.iter().enumerate() {
        // `!` reads as `i` only inside a word (`b!tch`), not as punctuation.
        let inside = |next: Option<&char>| {
            out.chars().last().is_some_and(char::is_alphabetic)
                && next.is_some_and(|n| n.is_alphabetic() || n.is_ascii_digit())
        };
        let c = match c {
            '!' if inside(chars.get(at + 1)) => 'i',
            '0' => 'o',
            '1' => 'i',
            '3' => 'e',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            '7' => 't',
            '8' => 'b',
            other => other,
        };
        if !collapse_runs || !out.ends_with(c) || !c.is_alphabetic() {
            out.push(c);
        }
    }
    out
}

/// Blanks number tokens (digits with `.`/`,`, optionally one `k`/`m`/`b`
/// unit: `800b`, `8008`, `1.5k`) so meso amounts and counts never read as
/// leetspeak (`800b` → `boob`); digits inside a word (`5h1t`) still map.
fn without_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let token = |c: char| c.is_alphanumeric() || c == '.' || c == ',';
        if !token(chars[at]) {
            out.push(chars[at]);
            at += 1;
            continue;
        }
        let end = (at..chars.len())
            .find(|&i| !token(chars[i]))
            .unwrap_or(chars.len());
        let word = &chars[at..end];
        let trimmed = {
            let mut len = word.len();
            while len > 0 && matches!(word[len - 1], '.' | ',') {
                len -= 1;
            }
            &word[..len]
        };
        let body = match trimmed.last() {
            Some(c) if matches!(c.to_ascii_lowercase(), 'k' | 'm' | 'b') => {
                &trimmed[..trimmed.len() - 1]
            }
            _ => trimmed,
        };
        let numeric = body.first().is_some_and(char::is_ascii_digit)
            && body
                .iter()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | ','));
        if numeric {
            out.extend(std::iter::repeat_n(' ', word.len()));
        } else {
            out.extend(word);
        }
        at = end;
    }
    out
}

/// Letter runs: `boob` → `[(b, 1), (o, 2), (b, 1)]`.
fn runs(word: &str) -> Vec<(char, usize)> {
    let mut out: Vec<(char, usize)> = Vec::new();
    for c in word.chars() {
        match out.last_mut() {
            Some((last, count)) if *last == c => *count += 1,
            _ => out.push((c, 1)),
        }
    }
    out
}

/// `raw` starts with `entry`'s runs, each at least as long: `booob` covers
/// `boob`, but `bob` does not (collapsing must not make another word).
fn covers(raw: &str, entry: &str) -> bool {
    let (raw, entry) = (runs(raw), runs(entry));
    raw.len() >= entry.len()
        && raw
            .iter()
            .zip(&entry)
            .all(|((letter, count), (want, need))| letter == want && count >= need)
}

/// Short entries that are common names or words themselves, or whose suffix
/// forms are (`asus`): chat matches them only as the exact whole word.
/// `tai`: a common surname/given name and an everyday Malay/Thai word;
/// `asu`: `asus` (the brand) and ASU. Other short entries (`fuk`, `tit`,
/// `cum`, `hoe`, `wtf`, `diu`, `kuy`, `yed`…) keep suffixes and stretching:
/// their forms are what members type (`fukin`, `tits`, `wtfff`).
pub const EXACT_ONLY: &[&str] = &["tai", "asu"];

/// Entries whose collapsed form is a common word or name (`boob` → `bob`,
/// `nigger` → `niger`): chat needs each letter run at least as long as the
/// entry's, so collapsing them never makes the other word. Collapse still
/// applies to every other entry (`ashole`, `pusy`, `jiz`, `chebai`). The
/// inside entry `niger` still matches "Niger"/"Nigeria" (accepted gap;
/// admins can allow it again).
pub const RUN_STRICT: &[&str] = &["boob", "nigger"];

/// Ordinary words and names a suffix form would otherwise hit
/// (`cum`+`in`, `hoe`+`y`, `cock`+`y`, `dik`+`es`, `babi`+`es`, collapsed
/// `titter` = `tit`+`er`).
pub const SAFE_FORMS: &[&str] = &[
    "cumin",
    "cumins",
    "hoey",
    "hoed",
    "hoer",
    "cocky",
    "cocker",
    "cockers",
    "cocking",
    "titter",
    "titters",
    "tittering",
    "tittered",
    "titer",
    "titers",
    "dikes",
    "babies",
];

fn collapse(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

static BUILTIN: LazyLock<WordFilter> = LazyLock::new(|| WordFilter::new(&[], &[]));

/// A deny-list hit: the built-in entry, or an admin-added word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit<'a> {
    Builtin(&'static str),
    Extra(&'a str),
}

impl Hit<'_> {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Builtin(word) => word,
            Self::Extra(word) => word,
        }
    }
}

/// Every built-in entry once, sorted: the words an admin may allow again.
/// Allowing one removes it from every list it is on (whole-word and inside).
pub fn builtin_words() -> Vec<&'static str> {
    let mut words: Vec<&'static str> = DENY_LIST
        .iter()
        .chain(DENY_SOUNDALIKE)
        .chain(DENY_SEA)
        .chain(DENY_INSIDE)
        .copied()
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

pub fn is_builtin_word(word: &str) -> bool {
    DENY_LIST
        .iter()
        .chain(DENY_SOUNDALIKE)
        .chain(DENY_SEA)
        .chain(DENY_INSIDE)
        .any(|entry| *entry == word)
}

/// The effective deny-list: built-ins minus `allowed`, plus admin `extra`
/// words (whole-word, with the built-in suffixes). Entries are collapsed once.
#[derive(Clone, Debug)]
pub struct WordFilter {
    builtin_whole: Vec<(String, &'static str)>,
    extra_whole: Vec<(String, String)>,
    inside: Vec<&'static str>,
}

impl WordFilter {
    pub fn new(extra: &[String], allowed: &[String]) -> Self {
        let kept = |entry: &&&'static str| !allowed.iter().any(|word| word == **entry);
        Self {
            builtin_whole: DENY_LIST
                .iter()
                .chain(DENY_SOUNDALIKE)
                .chain(DENY_SEA)
                .filter(kept)
                .map(|entry| (collapse(entry), *entry))
                .collect(),
            extra_whole: extra
                .iter()
                .map(|word| (collapse(&word.to_lowercase()), word.clone()))
                .collect(),
            inside: DENY_INSIDE.iter().filter(kept).copied().collect(),
        }
    }

    /// The built-in list with nothing allowed again and no extras.
    pub fn builtin() -> &'static Self {
        &BUILTIN
    }

    /// The chat guardrail's match (user reviews 2026-10-05): [`Self::denied`]
    /// with precise exceptions for words members really use: [`EXACT_ONLY`]
    /// entries match only exactly, [`RUN_STRICT`] entries need their letter
    /// runs, and [`SAFE_FORMS`] never hit. [`Self::denied`] keeps the nudge
    /// vectors' reading.
    pub fn denied_strict(&self, line: &str) -> Option<Hit<'_>> {
        let raw = leet(&without_numbers(line), false);
        raw.split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty())
            .filter(|raw_word| !SAFE_FORMS.contains(raw_word))
            .find_map(|raw_word| {
                let word = collapse(raw_word);
                let matches = |collapsed: &str, entry: &str| {
                    if EXACT_ONLY.contains(&entry) {
                        return raw_word == entry;
                    }
                    let whole = word == collapsed
                        || SUFFIXES.iter().any(|suffix| {
                            word.strip_prefix(collapsed)
                                .is_some_and(|rest| rest == collapse(suffix))
                        });
                    whole && (!RUN_STRICT.contains(&entry) || covers(raw_word, entry))
                };
                self.builtin_whole
                    .iter()
                    .find(|(collapsed, entry)| matches(collapsed, entry))
                    .map(|(_, entry)| Hit::Builtin(entry))
                    .or_else(|| {
                        self.extra_whole
                            .iter()
                            .find(|(collapsed, entry)| matches(collapsed, entry))
                            .map(|(_, entry)| Hit::Extra(entry.as_str()))
                    })
                    .or_else(|| {
                        self.inside
                            .iter()
                            .find(|entry| word.contains(*entry))
                            .map(|entry| Hit::Builtin(entry))
                    })
            })
    }

    /// The first deny-listed word in `line`, or `None`. Only the entry is
    /// reported, never the line.
    pub fn denied(&self, line: &str) -> Option<Hit<'_>> {
        let normalised = normalise(line);
        normalised
            .split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty())
            .find_map(|word| {
                let matches = |entry: &str| {
                    word == entry
                        || SUFFIXES.iter().any(|suffix| {
                            word.strip_prefix(entry)
                                .is_some_and(|rest| rest == collapse(suffix))
                        })
                };
                self.builtin_whole
                    .iter()
                    .find(|(collapsed, _)| matches(collapsed))
                    .map(|(_, entry)| Hit::Builtin(entry))
                    .or_else(|| {
                        self.extra_whole
                            .iter()
                            .find(|(collapsed, _)| matches(collapsed))
                            .map(|(_, word)| Hit::Extra(word.as_str()))
                    })
                    .or_else(|| {
                        self.inside
                            .iter()
                            .find(|entry| word.contains(*entry))
                            .map(|entry| Hit::Builtin(entry))
                    })
            })
    }
}

/// A built-in deny-listed word, or `None`. Only the entry is reported, never the line.
pub fn denied_word(line: &str) -> Option<&'static str> {
    match BUILTIN.denied(line)? {
        Hit::Builtin(word) => Some(word),
        Hit::Extra(_) => None,
    }
}
