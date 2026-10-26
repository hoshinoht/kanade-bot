use kanade_extract::gate::{self, BossLexicon};

use crate::support::fixture_catalog;

#[test]
fn the_gate_reads_bosses_days_times_and_mentions() {
    let table = fixture_catalog();
    let lexicon = BossLexicon::new(&table);
    let roster = ["11", "22", "33", "44"];
    let evaluate = |text: &str| gate::evaluate(text, &lexicon, &roster);

    let chatter = evaluate("lol that ring price is crazy");
    assert!(!chatter.hit());
    assert_eq!(chatter.reasons(), "-");

    let soon = evaluate("@here 9:30 later tonight");
    assert_eq!(soon.reasons(), "day,here,soon,time");
    assert!(soon.strong());
    assert_eq!(soon.times, ["9:30"]);
    assert_eq!(soon.days, ["tonight"]);

    let ping = evaluate("<@11> hstar tmr");
    assert_eq!(ping.reasons(), "boss,day,mention");
    assert_eq!(ping.bosses[0].canonical.as_deref(), Some("HMaleficStar"));
    assert_eq!(ping.mentions, ["11"]);
    assert!(!evaluate("<@99> hi").hit());

    let agreed = [evaluate("ok"), evaluate("Can")];
    assert!(!gate::should_extract(&agreed, false));
    assert!(gate::should_extract(&agreed, true));
}

/// Named known difference K-WORD-MARKS: Rust `regex`'s `\w` admits combining
/// marks such as the emoji variation selector U+FE0F, Python's (`isalnum`)
/// does not, so no `\b` falls between `❤️` and the digits. v4 finds `930`
/// here; v5 finds nothing. Plain emoji and custom-emoji markup are unaffected.
#[test]
fn known_difference_marks_are_word_characters() {
    assert_eq!(gate::find_times("❤️930"), Vec::<String>::new());
    assert_eq!(gate::find_times("❤ 930"), ["930"]);
    assert_eq!(gate::find_times("🔥930"), ["930"]);
}
