//! Static comma-separated word lists behind [`super::CodeLexicon`]. Lexicon
//! terms block pool names by near-match; stopwords only by equality.

pub(super) const CALENDAR: &str = "\
    monday, tuesday, wednesday, thursday, friday, saturday, sunday, mon, tue, tues, wed, thu, \
    thur, thurs, fri, sat, sun, today, tonight, tomorrow, yesterday, weekend, weekday, week, \
    january, february, march, april, may, june, july, august, september, october, november, \
    december, jan, feb, mar, apr, jun, jul, aug, sep, sept, oct, nov, dec, reset";

pub(super) const DIFFICULTY: &str = "\
    easy, normal, norm, hard, chaos, extreme, ex, p1, p2, p3, p4";

/// MapleStory classes, NPCs and bosses beyond the loaded catalog.
pub(super) const GAME: &str = "\
    hero, paladin, dark knight, arch mage, bishop, bowmaster, marksman, pathfinder, \
    night lord, shadower, dual blade, buccaneer, corsair, cannoneer, jett, dawn warrior, \
    blaze wizard, wind archer, night walker, thunder breaker, mihile, aran, evan, mercedes, \
    phantom, luminous, shade, eunwol, blaster, battle mage, wild hunter, mechanic, xenon, \
    demon slayer, demon avenger, kaiser, kain, cadena, angelic buster, illium, ark, adele, \
    khali, hoyoung, lara, kinesis, zero, beast tamer, kanna, hayato, ren, len, lynn, mo xuan, \
    sia astelle, haku, akechi, mitsuhide, nobunaga, hideyoshi, ieyasu, sakuno, ranmaru, \
    ayame, cygnus, hilla, lucid, will, gloom, darknell, verus, dusk, dunkel, magnus, zakum, \
    horntail, pink bean, papulatus, vellum, pierre, von bon, crimson queen, arkarium, \
    von leon, damien, slime, kai, kaling, gollux, ursus, lotus, seren, kalos, limbo, baldrix, \
    jupiter, bellona, tenebris, arcane, sacred, grandis, lachelein, esfera, cernium, odium, \
    arcus, arteria, carcion, tallahart, shangrila, moonbridge, legion, maple, maplestory, \
    erda, sol, hexa, meso, mesos";

/// Words that appear in code-owned prompt text and schemas.
pub(super) const SCHEMA: &str = "\
    participant, participants, amendment, amendments, member, members, boss, bosses, slot, \
    slots, party, guild, run, runs, kanade";

/// Common English (and English-borrowed Japanese) words a romanised name
/// could be mistaken for.
pub(super) const STOPWORDS: &str = "\
    a, able, about, after, again, ago, ai, aim, air, all, also, am, amen, ami, an, and, \
    anime, any, are, area, aria, arm, art, as, ask, at, aura, away, back, bad, bake, ban, \
    banana, bar, base, be, bed, been, best, big, bit, bonsai, book, boy, but, buy, by, cafe, \
    cake, call, came, can, car, care, case, cat, chi, coco, come, cool, cute, dam, dan, dash, \
    day, did, die, dim, do, does, dog, done, dot, down, each, ear, eat, emo, emoji, end, era, \
    even, ever, eye, fan, far, fast, few, fine, fire, for, from, fun, game, gave, get, go, \
    god, good, got, had, haiku, has, hat, have, he, her, here, hi, hide, him, his, hit, hot, \
    how, i, if, in, into, is, it, its, jam, job, joke, just, kami, kana, kanji, karaoke, \
    karate, karma, keep, ken, key, kid, kimono, kin, kind, kiwi, koi, lake, last, late, let, \
    like, line, lol, long, look, lot, made, mai, main, make, maki, mama, man, mana, manga, \
    many, mat, mate, me, meme, men, mid, mind, mine, miso, moe, momo, more, most, much, must, \
    my, name, nana, near, need, new, next, nice, nine, ninja, no, none, nori, not, now, of, \
    off, oh, ok, okay, old, on, once, one, only, or, origami, other, our, out, over, own, \
    papa, pet, play, put, ramen, ran, rate, real, rice, ride, rin, rise, role, room, sake, \
    sama, same, samurai, san, saw, say, see, sensei, set, she, shin, shy, side, so, some, \
    soon, sorry, stay, still, such, sumo, sure, sushi, take, tame, tan, tank, taro, tea, \
    team, ten, than, that, the, them, then, there, they, this, tie, time, to, tofu, tone, \
    too, top, toy, try, tsunami, tuna, two, udon, umami, up, us, use, very, was, way, we, \
    well, went, were, what, when, who, why, win, with, yay, yen, yes, yet, yo, yoga, you, \
    your, zen";
