//! Extra checks for model-written lead-ins (seed lines are human-approved and
//! skip them): no Discord markdown, no invisible format characters, no invite
//! links, and a small code-owned SFW deny-list (user decision 2026-09-25).

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
        if !out.ends_with(c) || !c.is_alphabetic() {
            out.push(c);
        }
    }
    out
}

fn collapse(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// A deny-listed word, or `None`. Only the entry is reported, never the line.
pub fn denied_word(line: &str) -> Option<&'static str> {
    let normalised = normalise(line);
    normalised
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| !word.is_empty())
        .find_map(|word| {
            let mut whole_words = DENY_LIST
                .iter()
                .chain(DENY_SOUNDALIKE)
                .chain(DENY_SEA)
                .copied();
            let whole = whole_words.find(|entry| {
                let entry = collapse(entry);
                word == entry
                    || SUFFIXES.iter().any(|suffix| {
                        word.strip_prefix(entry.as_str())
                            .is_some_and(|rest| rest == collapse(suffix))
                    })
            });
            whole.or_else(|| {
                DENY_INSIDE
                    .iter()
                    .copied()
                    .find(|entry| word.contains(entry))
            })
        })
}
