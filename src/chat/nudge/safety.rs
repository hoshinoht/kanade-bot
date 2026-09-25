//! Extra checks for model-written lead-ins (seed lines are human-approved and
//! skip them): no Discord markdown, no invisible format characters, no invite
//! links, and a small code-owned SFW deny-list (user decision 2026-09-25).

/// Characters Discord renders as formatting.
const MARKUP_CHARS: [char; 6] = ['`', '*', '_', '~', '|', '\\'];

const INVITES: [&str; 3] = ["discord.gg/", "discord.com/invite", "discordapp.com/invite"];

/// Whole words (after normalisation), each also matched with a suffix from
/// [`SUFFIXES`]. Kept small and readable: false positives only cost the seed line.
pub const DENY_LIST: [&str; 35] = [
    "anal", "arse", "asshole", "bastard", "bitch", "blowjob", "boob", "cock", "cum", "cunt",
    "dick", "dildo", "erotic", "fap", "faggot", "fetish", "fuck", "hentai", "horny", "kinky",
    "lewd", "milf", "naked", "nigger", "nsfw", "nude", "orgasm", "penis", "porn", "pussy", "rape",
    "retard", "sex", "shit", "slut",
];

const SUFFIXES: [&str; 7] = ["s", "es", "ed", "er", "ing", "y", "ty"];

/// Markdown syntax at the start of the line or anywhere inline.
pub fn has_markup(line: &str) -> bool {
    let start = line.trim_start();
    let block = ["#", "-#", ">", "- ", "* ", "+ "]
        .iter()
        .any(|prefix| start.starts_with(prefix));
    let digits = start.chars().take_while(char::is_ascii_digit).count();
    let numbered = digits > 0 && start[digits..].starts_with(". ");
    block || numbered || line.contains(MARKUP_CHARS)
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
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
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
            DENY_LIST.iter().copied().find(|entry| {
                let entry = collapse(entry);
                word == entry
                    || SUFFIXES.iter().any(|suffix| {
                        word.strip_prefix(entry.as_str())
                            .is_some_and(|rest| rest == collapse(suffix))
                    })
            })
        })
}
