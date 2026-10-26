//! The estimate share chart: the SVG pinned against stored copies of the
//! approved board's fixtures, the PNG decoded for size and palette, and the
//! image on day-of and countdown posts (never on an edit, left off when
//! anything fails).
//!
//! `KANADE_WRITE_CHART=1` rewrites the stored SVGs; `KANADE_CHART_PNG_DIR`
//! also writes the rendered PNGs there for a visual check.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeDelta, TimeZone, Utc};
use kanade::cards::chart::{self, Chart, ChartRun, SCALE};
use kanade::cards::estimates::{
    CachedEstimates, EstimateFuture, IgnFuture, MAX_ATTACHMENTS, PartyEstimates, RunEstimate,
    attach_chart, chart_for, read_estimates,
};
use kanade::cards::{self, CardArt, CardContext, CardKit, V2Kit, redesign};
use kanade::discord::transport::{OutgoingMessage, Upload, legacy_rows};
use kanade::infrastructure::character_data::{CharacterData, RosterSide, Timing};
use kanade::infrastructure::scouter::{Card, Combined, Member};
use kanade_domain::catalog::BossTable;
use kanade_domain::characters::{Basis, BossSlot, Character, CharacterRole, PartySlot};
use kanade_domain::chardata::{BossCarrier, Carrier, CharDataStore, PartyShares};
use kanade_domain::history::Actor;
use kanade_domain::schedule::{Run, ScheduleSnapshot};
use kanade_store::{SqliteCharData, SqliteStore};
use resvg::tiny_skia::Pixmap;

use super::{KALOS, MALEFIC, countdown, day_of, members, redesigned_v5, week};
use crate::cards::{answer, art_dir, catalog, created, edits, kit, refresher, seed_day_of, world};
use crate::scenarios::{self, now};
use crate::support;

fn carrier(ign: &str, crit: bool, effective: &[f64]) -> Carrier {
    Carrier {
        ign: ign.into(),
        crit_assumed: crit,
        effective: effective.iter().copied().map(Some).collect(),
        per_boss: Vec::new(),
    }
}

fn blank(carriers: &[Carrier]) -> Vec<Carrier> {
    carriers
        .iter()
        .map(|carrier| Carrier {
            effective: vec![None; carrier.effective.len()],
            ..carrier.clone()
        })
        .collect()
}

fn run(
    time: &str,
    bosses: &[&str],
    carriers: Vec<Carrier>,
    missing: usize,
    minutes: &[f64],
) -> ChartRun {
    ChartRun {
        time: time.into(),
        party: PartyShares {
            bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
            carriers,
            missing,
        },
        minutes: minutes.iter().copied().map(Some).collect(),
    }
}

/// The board's Kalos run (the user's screenshot numbers, invented IGNs).
fn kalos() -> ChartRun {
    run(
        "21:30",
        &["XKalos"],
        vec![
            carrier("AsahiDK", false, &[38.8]),
            carrier("RenShade", true, &[35.8]),
            carrier("MikaNL", false, &[25.3]),
            carrier("TsubaKain", true, &[23.7]),
            carrier("SoraPF", false, &[16.8]),
        ],
        0,
        &[21.4],
    )
}

/// The board's Karling + Star run: five carriers, two with no character.
fn carling() -> ChartRun {
    run(
        "22:00",
        &["HCarling", "HMaleficStar"],
        vec![
            carrier("AsahiDK", false, &[34.0, 41.0]),
            carrier("RenShade", false, &[22.5, 28.0]),
            carrier("MikaBish", true, &[18.0, 21.5]),
            carrier("YuzuPala", false, &[20.5, 24.0]),
            carrier("TsubaKain", false, &[17.0, 16.0]),
        ],
        2,
        &[26.8, 22.9],
    )
}

/// `carling`'s party at other bosses, each figure scaled.
fn scaled(
    time: &str,
    bosses: [&str; 2],
    factors: [(usize, f64); 2],
    minutes: [f64; 2],
) -> ChartRun {
    let base = carling();
    let carriers = base
        .party
        .carriers
        .iter()
        .map(|carrier| Carrier {
            effective: factors
                .iter()
                .map(|(from, factor)| carrier.effective[*from].map(|value| value * factor))
                .collect(),
            ..carrier.clone()
        })
        .collect();
    run(time, &bosses, carriers, 2, &minutes)
}

const AS_OF: &str = "Tue 29 Sep 08:41";

/// The board's PNGs: (file stem, chart).
fn fixtures() -> Vec<(&'static str, Chart)> {
    let chart = |runs: Vec<ChartRun>, as_of: &str| Chart {
        runs,
        as_of: as_of.into(),
        older_than: None,
        portal: true,
    };
    let mut jupiter = kalos();
    jupiter.time = "23:59".into();
    jupiter.party.bosses = vec!["HJupiter".into()];
    let mut none_kalos = kalos();
    none_kalos.party.carriers = blank(&none_kalos.party.carriers);
    let mut none_carling = carling();
    none_carling.party.carriers = blank(&none_carling.party.carriers);
    vec![
        ("estimate-share-A-kalos", chart(vec![kalos()], AS_OF)),
        (
            "estimate-share-A-tue29",
            chart(vec![kalos(), carling()], AS_OF),
        ),
        (
            "estimate-share-A-busy",
            chart(
                vec![
                    kalos(),
                    carling(),
                    scaled(
                        "22:30",
                        ["HLimbo", "HBaldrix"],
                        [(0, 0.93), (1, 0.77)],
                        [28.9, 29.6],
                    ),
                    scaled(
                        "23:30",
                        ["XBM", "HSeren"],
                        [(0, 0.857), (0, 1.348)],
                        [31.3, 19.9],
                    ),
                    jupiter,
                ],
                AS_OF,
            ),
        ),
        (
            "estimate-share-A-none",
            chart(vec![none_kalos, none_carling], "Thu 1 Oct 09:00"),
        ),
    ]
}

fn golden(stem: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/delivery/redesign/chart")
        .join(format!("{stem}.svg"))
}

/// The pixel at CSS px (`x`, `y`) as `#rrggbb`.
fn colour(png: &Pixmap, x: u32, y: u32) -> String {
    let pixel = png.pixel(x * SCALE, y * SCALE).expect("inside the image");
    format!(
        "#{:02x}{:02x}{:02x}",
        pixel.red(),
        pixel.green(),
        pixel.blue()
    )
}

#[test]
fn the_board_fixtures_draw_as_stored_and_decode_at_twice_the_size() {
    let out = std::env::var_os("KANADE_CHART_PNG_DIR").map(PathBuf::from);
    for (stem, fixture) in fixtures() {
        let drawn = chart::svg(&fixture);
        assert_eq!(chart::svg(&fixture), drawn, "{stem}: deterministic");
        if std::env::var_os("KANADE_WRITE_CHART").is_some() {
            std::fs::create_dir_all(golden(stem).parent().unwrap()).unwrap();
            std::fs::write(golden(stem), &drawn.svg).unwrap();
        }
        let stored = std::fs::read_to_string(golden(stem)).expect("stored svg");
        assert_eq!(drawn.svg, stored, "{stem}");

        let bytes = chart::png(&drawn).expect("renders");
        assert_eq!(
            chart::png(&drawn).unwrap(),
            bytes,
            "{stem}: same bytes twice"
        );
        let png = Pixmap::decode_png(&bytes).expect("a PNG");
        assert_eq!(
            (png.width(), png.height()),
            (drawn.width * SCALE, drawn.height * SCALE),
            "{stem}"
        );
        assert_eq!(png.width(), 1120);
        assert_eq!(colour(&png, 2, 2), "#2b2d31", "{stem}: opaque dark ground");
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("{stem}.png")), &bytes).unwrap();
        }
    }
    let kalos = chart::svg(&fixtures()[0].1);
    // The board's heights.
    assert_eq!(kalos.height, 160);
    assert_eq!(chart::svg(&fixtures()[1].1).height, 334);
    assert_eq!(chart::svg(&fixtures()[2].1).height, 800);
    assert_eq!(chart::svg(&fixtures()[3].1).height, 226);
    // Member colours at the parts' left edges (party order), on the bar
    // at y 38–66: AsahiDK blue, MikaNL green; RenShade is hatched.
    let png = Pixmap::decode_png(&chart::png(&kalos).unwrap()).unwrap();
    assert_eq!(colour(&png, 112, 42), "#3a9ad9");
    assert_eq!(colour(&png, 343, 42), "#14b386");
}

#[test]
fn the_svg_says_what_the_board_says() {
    let drawn: Vec<String> = fixtures()
        .iter()
        .map(|(_, fixture)| chart::svg(fixture).svg)
        .collect();
    let [one, tue29, busy, none] = drawn.as_slice() else {
        panic!("four fixtures");
    };
    for part in [
        "1·28%",
        "2·25%*",
        "3·18%",
        "4·17%*",
        "5·12%",
        "140.4% · 21.4m",
    ] {
        assert!(one.contains(part), "{part}");
    }
    assert!(one.contains("as of Tue 29 Sep 08:41 · * assumes 100% critical rate"));
    assert!(!one.contains("with no estimate"));
    assert!(tue29.contains("22:00 · HCarling + HMaleficStar"));
    assert!(tue29.contains("2 with no estimate (not 0%, not in the total)"));
    assert!(busy.contains("23:59 · HJupiter"), "eight bars fit");
    assert!(!busy.contains(" more boss"));
    // A ninth bar and the runs after it are counted, not drawn.
    let mut over = fixtures().swap_remove(2).1;
    over.runs.push(kalos());
    over.runs.push(carling());
    let drawn = chart::svg(&over).svg;
    assert!(drawn.contains("+3 more bosses in the portal"), "{drawn}");
    over.portal = false;
    assert!(chart::svg(&over).svg.contains("+3 more bosses</text>"));
    over.runs.truncate(6);
    assert!(chart::svg(&over).svg.contains(">+1 more boss</text>"));
    assert_eq!(none.matches("no estimate yet").count(), 3);
    assert!(
        !none.contains("with no estimate"),
        "no legend without a figure"
    );
    assert!(
        none.contains("as of Thu 1 Oct 09:00</text>"),
        "no crit note"
    );
    // IGNs only, escaped.
    let odd = Chart {
        runs: vec![run(
            "21:30",
            &["XKalos"],
            vec![carrier("A<b&", false, &[1.0])],
            0,
            &[],
        )],
        as_of: AS_OF.into(),
        older_than: None,
        portal: false,
    };
    assert!(chart::svg(&odd).svg.contains("A&lt;b&amp;"));
    assert!(chart::png(&chart::svg(&odd)).is_ok());
}

#[test]
fn the_alt_text_carries_the_same_facts_by_ign() {
    let fixtures = fixtures();
    assert_eq!(
        chart::alt(&fixtures[0].1),
        "21:30 XKalos: XKalos 140.4%, AsahiDK 28%, RenShade 25% (assumed crit), MikaNL 18%, \
         TsubaKain 17% (assumed crit), SoraPF 12%. As of Tue 29 Sep 08:41."
    );
    assert!(chart::alt(&fixtures[1].1).contains("; 2 with no estimate. As of"));
    assert_eq!(
        chart::alt(&fixtures[3].1),
        "21:30 XKalos: XKalos no estimate yet. 22:00 HCarling + HMaleficStar: HCarling no \
         estimate yet; HMaleficStar no estimate yet. As of Thu 1 Oct 09:00."
    );
}

/// The oldest reading at least `estimate_days` old: the "as of" line and
/// the alt text say "older than n days" (one day singular); fresh, both
/// are as the board has them.
#[test]
fn a_stale_chart_says_older_than_n_days_and_a_fresh_one_does_not() {
    let fresh = fixtures().remove(0).1;
    let drawn = chart::svg(&fresh).svg;
    assert!(
        drawn.contains(&format!(
            "Clear-time estimate · as of {AS_OF} · * assumes 100% critical rate</text>"
        )),
        "{drawn}"
    );
    assert!(!drawn.contains("older than"));
    assert!(chart::alt(&fresh).ends_with(&format!(". As of {AS_OF}.")));

    let out = std::env::var_os("KANADE_CHART_PNG_DIR").map(PathBuf::from);
    for (days, phrase) in [(7, "older than 7 days"), (1, "older than 1 day")] {
        let stale = Chart {
            older_than: Some(days),
            ..fresh.clone()
        };
        let drawn = chart::svg(&stale);
        assert!(
            drawn.svg.contains(&format!(
                "Clear-time estimate · as of {AS_OF} · {phrase} · * assumes 100% critical rate</text>"
            )),
            "{}",
            drawn.svg
        );
        let alt = chart::alt(&stale);
        assert!(
            alt.ends_with(&format!(". As of {AS_OF} · {phrase}.")),
            "{alt}"
        );
        assert_eq!(drawn.height, chart::svg(&fresh).height, "the same line");
        let bytes = chart::png(&drawn).expect("renders");
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("estimate-stale-{days}.png")), &bytes).unwrap();
        }
    }
}

fn character(member: &str, ign: &str, world: &str) -> Character {
    let at = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
    Character {
        id: format!("ch-{member}"),
        member_id: member.into(),
        ign: ign.into(),
        world: world.into(),
        class: None,
        level: None,
        role: CharacterRole::Main,
        retired_at: None,
        created_at: at,
        updated_at: at,
        updated_by: Actor::system("test"),
        version: 1,
    }
}

fn slot(member: &str, brings: Option<Character>) -> PartySlot {
    PartySlot {
        member_id: member.into(),
        brings: brings
            .as_ref()
            .map(|character| (character.id.clone(), Basis::Main)),
        candidates: brings.into_iter().collect(),
        retired: 0,
        pick: None,
        version: 0,
        clashes: Vec::new(),
        bosses: Vec::new(),
    }
}

fn reading(ign: &str, effective: Option<f64>, percentage: f64, crit: bool) -> Member {
    Member {
        ign: ign.into(),
        preset: "00010".into(),
        critical_rate_assumed: crit,
        critical_rate_actual: crit.then_some(84.0),
        source: None,
        bosses: [(
            "extreme_kalos".to_owned(),
            Card {
                percentage,
                rating: "Party-able".into(),
                party: false,
                effective_percentage: effective,
                estimated_minutes: None,
            },
        )]
        .into(),
    }
}

/// The party is read from the slots and the estimates memory only: no
/// character or an IGN the scouter can't take is missing; a character
/// with nothing cached keeps their number without a figure; a card's
/// whole-party percentage never stands in for a missing effective one.
#[test]
fn a_run_reads_its_party_from_slots_and_the_estimates_memory() {
    let schedule = week();
    let kalos = schedule.runs.iter().find(|run| run.id == KALOS).unwrap();
    let mut run = kalos.clone();
    run.bosses = vec!["XKalos".into()];
    let fetched = |hour| Utc.with_ymd_and_hms(2026, 9, 9, hour, 0, 0).unwrap();
    let slots = [
        slot("1001", Some(character("1001", "AsahiDK", "msea"))),
        slot("1002", None),
        slot("1003", Some(character("1003", "RenShade", "msea"))),
        slot("1004", Some(character("1004", "Overseas", "gms"))),
        slot("1005", Some(character("1005", "MikaNL", "msea"))),
        slot("1006", Some(character("1006", "SoraPF", "msea"))),
    ];
    let party_asked = AtomicUsize::new(0);
    let estimate = read_estimates(
        &run,
        &slots,
        |ign| match ign.as_str() {
            "AsahiDK" => Some((reading("AsahiDK", Some(38.8), 140.5, false), fetched(8))),
            "RenShade" => Some((reading("RenShade", Some(35.8), 140.5, true), fetched(6))),
            "MikaNL" => Some((reading("MikaNL", None, 140.5, false), fetched(7))),
            _ => None,
        },
        |_| {
            party_asked.fetch_add(1, Ordering::SeqCst);
            None
        },
    );
    assert_eq!(
        estimate.party,
        PartyShares {
            bosses: vec!["XKalos".into()],
            carriers: vec![
                carrier("AsahiDK", false, &[38.8]),
                carrier("RenShade", true, &[35.8]),
                Carrier {
                    effective: vec![None],
                    ..carrier("MikaNL", false, &[])
                },
                Carrier {
                    effective: vec![None],
                    ..carrier("SoraPF", false, &[])
                },
            ],
            missing: 2,
        }
    );
    assert_eq!(estimate.oldest, Some(fetched(6)));
    assert_eq!(estimate.minutes, vec![None]);
    assert_eq!(
        party_asked.load(Ordering::SeqCst),
        0,
        "a party total only for a whole party"
    );
    assert_eq!(estimate.party.no_estimate(), 4);
}

/// A reading with a card for XKalos and HMaleficStar.
fn two(ign: &str, kalos: f64, star: f64) -> Member {
    let card = |effective: f64| Card {
        percentage: effective,
        rating: "Party-able".into(),
        party: false,
        effective_percentage: Some(effective),
        estimated_minutes: None,
    };
    Member {
        bosses: [
            ("extreme_kalos".to_owned(), card(kalos)),
            ("hard_maleficStar".to_owned(), card(star)),
        ]
        .into(),
        ..reading(ign, None, 0.0, false)
    }
}

/// A total of `minutes` for each of the two bosses' keys.
fn total(minutes: f64) -> std::collections::BTreeMap<String, Combined> {
    let combined: Combined = serde_json::from_value(serde_json::json!({
        "combined_percentage": 120.0,
        "estimated_minutes": minutes,
        "technically_possible": true,
        "complete": true,
        "issues": {},
    }))
    .expect("a total");
    [
        ("extreme_kalos".to_owned(), combined.clone()),
        ("hard_maleficStar".to_owned(), combined),
    ]
    .into()
}

/// Per-boss picks: each bar is that boss's own party (a member keeps their
/// number with another character), each boss's minutes come from its own
/// party's total, and only then does the legend name the characters per
/// bar.
#[test]
fn each_bar_is_its_own_bosss_party_when_the_party_differs() {
    let schedule = week();
    let mut run = schedule
        .runs
        .iter()
        .find(|run| run.id == KALOS)
        .unwrap()
        .clone();
    run.bosses = vec!["XKalos".into(), "HMaleficStar".into()];
    let fetched = Utc.with_ymd_and_hms(2026, 9, 9, 8, 0, 0).unwrap();
    let dk = character("1001", "AsahiDK", "msea");
    let mech = Character {
        id: "ch-1001-mech".into(),
        ign: "AsahiMech".into(),
        role: CharacterRole::Alt,
        ..dk.clone()
    };
    let boss = |token: &str, key: &str, brings: &Character, basis| BossSlot {
        token: token.into(),
        boss: key.into(),
        pick: None,
        brings: Some((brings.id.clone(), basis)),
        version: 0,
    };
    let asahi = PartySlot {
        candidates: vec![dk.clone(), mech.clone()],
        bosses: vec![
            boss("XKalos", "Kalos", &dk, Basis::Main),
            boss("HMaleficStar", "MaleficStar", &mech, Basis::RunPick),
        ],
        ..slot("1001", Some(dk.clone()))
    };
    let ren = slot("1003", Some(character("1003", "RenShade", "msea")));
    let asked = std::cell::RefCell::new(Vec::new());
    let estimate = read_estimates(
        &run,
        &[asahi, ren],
        |ign| match ign.as_str() {
            "AsahiDK" => Some((two("AsahiDK", 40.0, 10.0), fetched)),
            "AsahiMech" => Some((two("AsahiMech", 5.0, 30.0), fetched)),
            "RenShade" => Some((two("RenShade", 60.0, 90.0), fetched)),
            _ => None,
        },
        |igns| {
            let set: Vec<String> = igns.iter().map(|ign| ign.as_str().to_owned()).collect();
            asked.borrow_mut().push(set.clone());
            let minutes = if set.contains(&"AsahiMech".to_owned()) {
                9.0
            } else {
                7.0
            };
            Some((total(minutes), fetched))
        },
    );
    assert_eq!(
        *asked.borrow(),
        [
            vec!["AsahiDK".to_owned(), "RenShade".to_owned()],
            vec!["AsahiMech".to_owned(), "RenShade".to_owned()],
        ],
        "one total per distinct boss party"
    );
    assert_eq!(estimate.minutes, [Some(7.0), Some(9.0)]);
    let party = &estimate.party;
    assert_eq!(
        party.carriers[0],
        Carrier {
            per_boss: vec![
                None,
                Some(BossCarrier {
                    ign: "AsahiMech".into(),
                    crit_assumed: false,
                }),
            ],
            ..carrier("AsahiDK", false, &[40.0, 30.0])
        }
    );
    assert_eq!(party.carriers[1], carrier("RenShade", false, &[60.0, 90.0]));
    assert_eq!(party.shares(1), [Some(25), Some(75)]);
    let chart = Chart {
        runs: vec![ChartRun {
            time: "21:30".into(),
            party: estimate.party.clone(),
            minutes: estimate.minutes.clone(),
        }],
        as_of: "Tue 29 Sep 08:41".into(),
        older_than: None,
        portal: false,
    };
    let drawn = chart::svg(&chart).svg;
    assert!(
        drawn.contains(">AsahiMech<"),
        "the Star bar names AsahiMech"
    );
    assert!(drawn.contains(">AsahiDK<"));
    assert_eq!(drawn.matches(">RenShade<").count(), 2, "once per bar");
    let alt = chart::alt(&chart);
    assert!(
        alt.contains("HMaleficStar 120.0%, AsahiMech 25%, RenShade 75%"),
        "{alt}"
    );
    assert!(
        alt.contains("XKalos 100.0%, AsahiDK 40%, RenShade 60%"),
        "{alt}"
    );
    // The same characters at every boss: one legend, as before.
    let same = Chart {
        runs: vec![ChartRun {
            party: PartyShares {
                carriers: vec![
                    carrier("AsahiDK", false, &[40.0, 10.0]),
                    carrier("RenShade", false, &[60.0, 90.0]),
                ],
                ..estimate.party
            },
            ..chart.runs[0].clone()
        }],
        ..chart
    };
    assert_eq!(chart::svg(&same).svg.matches(">RenShade<").count(), 1);
}

/// A port answering fixed estimates per run id; `None` for any other run.
struct Fixed(Vec<(String, RunEstimate)>);

impl PartyEstimates for Fixed {
    fn run<'a>(
        &'a self,
        run: &'a Run,
        _: &'a ScheduleSnapshot,
        _: &'a BossTable,
    ) -> EstimateFuture<'a> {
        let found = self
            .0
            .iter()
            .find(|(id, _)| *id == run.id)
            .map(|(_, estimate)| estimate.clone());
        Box::pin(async move { found })
    }

    /// Charts never read the roster's IGNs.
    fn igns<'a>(&'a self, _: &'a [String]) -> IgnFuture<'a> {
        Box::pin(async { None })
    }
}

fn estimate(time: ChartRun) -> RunEstimate {
    RunEstimate {
        party: time.party,
        minutes: time.minutes,
        oldest: Some(Utc.with_ymd_and_hms(2026, 9, 9, 0, 41, 0).unwrap()),
        older_than: None,
        ..RunEstimate::default()
    }
}

fn port() -> Arc<dyn PartyEstimates> {
    let mut star = carling();
    star.party.bosses = vec!["HMaleficStar".into(), "HKalos".into()];
    Arc::new(Fixed(vec![
        (KALOS.into(), estimate(kalos())),
        (MALEFIC.into(), estimate(star)),
    ]))
}

fn charted(kit: CardKit) -> CardKit {
    CardKit {
        estimates: Some(port()),
        ..kit
    }
}

fn post() -> OutgoingMessage {
    OutgoingMessage {
        content: Some("📅 **Today**".into()),
        embeds: Vec::new(),
        allowed_mentions: kanade::discord::mentions::none(),
        reply_to: None,
        attachments: Vec::new(),
        components: Vec::new(),
    }
}

fn png_upload(message: &OutgoingMessage) -> Option<&Upload> {
    message
        .attachments
        .iter()
        .find(|upload| upload.filename == chart::FILENAME)
}

#[tokio::test]
async fn redesigned_day_of_and_countdown_posts_carry_the_chart() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned_v5(&schedule, &roster, &table);
    let kit = charted(CardKit::default());
    let at = Utc.with_ymd_and_hms(2026, 9, 9, 1, 0, 0).unwrap();

    let both = chart_for(&day_of(&[KALOS, MALEFIC]), &ctx, &*port(), at)
        .await
        .expect("a chart");
    // Time order, as the card; the as-of is the oldest reading, guild time.
    assert_eq!(both.runs.len(), 2);
    assert_eq!(both.runs[1].party.bosses, ["XKalos"]);
    assert_eq!(both.as_of, "Wed 9 Sep 08:41");
    assert!(!both.portal, "no portal link without one");

    let mut message = post();
    attach_chart(&mut message, &countdown(KALOS), &ctx, &kit, at).await;
    let upload = png_upload(&message).expect("the chart");
    assert!(upload.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(
        upload
            .description
            .as_deref()
            .is_some_and(|alt| alt.contains("XKalos 140.4%, AsahiDK 28%")),
        "{:?}",
        upload.description
    );

    // Nobody with a reading still draws "no estimate yet".
    let mut none = kalos();
    none.party.carriers = blank(&none.party.carriers);
    let empty = Fixed(vec![(
        KALOS.into(),
        RunEstimate {
            oldest: None,
            ..estimate(none)
        },
    )]);
    let chart = chart_for(&countdown(KALOS), &ctx, &empty, at)
        .await
        .unwrap();
    assert_eq!(
        chart.as_of, "Wed 9 Sep 09:00",
        "post time without a reading"
    );
    assert!(chart::svg(&chart).svg.contains("no estimate yet"));

    let portal = CardKit {
        v2: V2Kit {
            portal: Some("https://portal.example".into()),
            portal_open: Some(Arc::new(|| true)),
            ..V2Kit::default()
        },
        ..kit.clone()
    };
    let ctx_portal = kanade::cards::CardContext {
        v2: Some(&portal.v2),
        ..ctx
    };
    assert!(
        chart_for(&countdown(KALOS), &ctx_portal, &*port(), at)
            .await
            .unwrap()
            .portal
    );
}

#[tokio::test]
async fn unset_ports_and_failures_post_without_it() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned_v5(&schedule, &roster, &table);
    let at = Utc.with_ymd_and_hms(2026, 9, 9, 1, 0, 0).unwrap();
    let attached = |message: &OutgoingMessage| png_upload(message).is_some();

    let mut message = post();
    attach_chart(
        &mut message,
        &day_of(&[KALOS]),
        &ctx,
        &CardKit::default(),
        at,
    )
    .await;
    assert!(!attached(&message), "estimates off");

    // A party that can't be read (the port's failure) leaves it off.
    let failing = CardKit {
        estimates: Some(Arc::new(Fixed(Vec::new()))),
        ..CardKit::default()
    };
    attach_chart(&mut message, &day_of(&[KALOS]), &ctx, &failing, at).await;
    assert!(!attached(&message), "unreadable party");
    assert!(
        chart_for(&day_of(&[KALOS, MALEFIC]), &ctx, &*port(), at)
            .await
            .is_some()
    );
    attach_chart(
        &mut message,
        &day_of(&[KALOS, "gone"]),
        &ctx,
        &CardKit {
            estimates: Some(Arc::new(Fixed(vec![(KALOS.into(), estimate(kalos()))]))),
            ..CardKit::default()
        },
        at,
    )
    .await;
    assert!(
        attached(&message),
        "a missing run is skipped as the card skips it"
    );

    // Discord's ten files: full, and the chart is left off.
    let mut full = post();
    full.attachments = (0..MAX_ATTACHMENTS)
        .map(|index| Upload {
            filename: format!("art-{index}.png"),
            bytes: Arc::from(&b"art"[..]),
            description: None,
        })
        .collect();
    attach_chart(
        &mut full,
        &day_of(&[KALOS]),
        &ctx,
        &charted(CardKit::default()),
        at,
    )
    .await;
    assert_eq!(full.attachments.len(), MAX_ATTACHMENTS);
    assert!(!attached(&full));

    // Kinds without runs never chart.
    let digest = kanade_domain::notify::IntentContent::Plain;
    assert!(chart_for(&digest, &ctx, &*port(), at).await.is_none());
}

/// One stale run marks the whole card's chart: the "as of" line and the
/// posted PNG's description say "older than n days". Fresh, neither does.
#[tokio::test]
async fn a_stale_reading_marks_the_posted_chart_and_its_description() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned_v5(&schedule, &roster, &table);
    let at = Utc.with_ymd_and_hms(2026, 9, 9, 1, 0, 0).unwrap();
    let content = day_of(&[KALOS, MALEFIC]);
    let stale = || {
        let mut star = carling();
        star.party.bosses = vec!["HMaleficStar".into(), "HKalos".into()];
        Fixed(vec![
            (KALOS.into(), estimate(kalos())),
            (
                MALEFIC.into(),
                RunEstimate {
                    older_than: Some(7),
                    ..estimate(star)
                },
            ),
        ])
    };
    let chart = chart_for(&content, &ctx, &stale(), at)
        .await
        .expect("a chart");
    assert_eq!(chart.older_than, Some(7));
    let drawn = chart::svg(&chart).svg;
    assert!(
        drawn.contains("as of Wed 9 Sep 08:41 · older than 7 days"),
        "{drawn}"
    );
    let stale_kit = CardKit {
        estimates: Some(Arc::new(stale())),
        ..CardKit::default()
    };
    let alt = described(&content, &ctx, &stale_kit, at).await;
    assert!(
        alt.ends_with(". As of Wed 9 Sep 08:41 · older than 7 days."),
        "{alt}"
    );

    let fresh = chart_for(&content, &ctx, &*port(), at).await.unwrap();
    assert_eq!(fresh.older_than, None);
    assert!(!chart::svg(&fresh).svg.contains("older than"));
    let alt = described(&content, &ctx, &charted(CardKit::default()), at).await;
    assert!(alt.ends_with(". As of Wed 9 Sep 08:41."), "{alt}");
}

/// The description of the chart a new post of `content` carries.
async fn described(
    content: &kanade_domain::notify::IntentContent,
    ctx: &CardContext<'_>,
    kit: &CardKit,
    at: chrono::DateTime<Utc>,
) -> String {
    let mut message = post();
    attach_chart(&mut message, content, ctx, kit, at).await;
    png_upload(&message)
        .and_then(|upload| upload.description.clone())
        .expect("the chart")
}

/// Estimates off (module `character_data` without a scouter URL): the
/// serve port draws nothing and the card posts exactly as it would have.
#[tokio::test]
async fn without_a_scouter_url_the_card_posts_unchanged() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned_v5(&schedule, &roster, &table);
    let at = Utc.with_ymd_and_hms(2026, 9, 9, 1, 0, 0).unwrap();
    let store = Arc::new(SqliteStore::open_temp().await);
    let module = CharacterData::new(
        Arc::new(SqliteCharData::open_temp().await) as Arc<dyn CharDataStore>,
        Arc::clone(&store) as Arc<dyn RosterSide>,
        None,
        Arc::new(move || at),
        Timing::default(),
    );
    assert!(module.estimates().is_none(), "no scouter URL");
    let port = CachedEstimates::new(store, Arc::new(module));
    let content = countdown(KALOS);
    assert!(chart_for(&content, &ctx, &port, at).await.is_none());

    let kit = CardKit {
        estimates: Some(Arc::new(port)),
        ..CardKit::default()
    };
    let card = cards::build(&content, &ctx, None, &[]).expect("a card");
    let mut message = card.message(&[], &CardArt::default());
    let before = message.clone();
    attach_chart(&mut message, &content, &ctx, &kit, at).await;
    assert_eq!(message, before, "posted as it would have been");
}

/// While the public portal is open the run cards post with the legacy
/// "Open in portal" row under their text: the chart is still attached
/// (Discord shows files beside link rows), and its overflow line points
/// at the portal. A V2 layout still has nowhere to show it.
#[tokio::test]
async fn an_open_portal_post_carries_the_chart_beside_its_link_row() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let at = Utc.with_ymd_and_hms(2026, 9, 9, 1, 0, 0).unwrap();
    let portal = V2Kit {
        portal: Some("https://portal.example".into()),
        portal_open: Some(Arc::new(|| true)),
        ..V2Kit::default()
    };
    let ctx = CardContext {
        v2: Some(&portal),
        ..redesigned_v5(&schedule, &roster, &table)
    };
    // Nine bosses: the ninth bar is counted on the overflow line.
    let bosses: Vec<String> = (1..=9).map(|n| format!("HBoss{n}")).collect();
    let busy = RunEstimate {
        party: PartyShares {
            bosses: bosses.clone(),
            carriers: vec![carrier("AsahiDK", false, &[30.0; 9])],
            missing: 0,
        },
        minutes: vec![None; 9],
        oldest: None,
        older_than: None,
        ..RunEstimate::default()
    };
    let port = Fixed(vec![(KALOS.into(), busy)]);
    let kit = CardKit {
        estimates: Some(Arc::new(Fixed(port.0.clone()))),
        v2: portal.clone(),
        ..CardKit::default()
    };
    let content = countdown(KALOS);
    let card = cards::build(&content, &ctx, None, &[]).expect("a card");
    let mut message = card.message(&[], &CardArt::default());
    assert!(
        !message.components.is_empty()
            && legacy_rows(message.content.as_deref(), &message.components),
        "the Open in portal row: {:?}",
        message.components
    );
    attach_chart(&mut message, &content, &ctx, &kit, at).await;
    let upload = png_upload(&message).expect("the chart beside the link row");
    assert!(upload.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    let chart = chart_for(&content, &ctx, &port, at).await.expect("a chart");
    assert!(chart.portal);
    let drawn = chart::svg(&chart).svg;
    assert!(drawn.contains("+1 more boss in the portal"), "{drawn}");

    // A V2 layout (no text, components only): left off as before.
    let mut v2 = OutgoingMessage::v2(
        vec![redesign::container(0, vec![redesign::text("📅 **Today**")])],
        kanade::discord::mentions::none(),
    );
    attach_chart(&mut v2, &content, &ctx, &kit, at).await;
    assert!(png_upload(&v2).is_none(), "a V2 layout shows no file");
}

/// Through the tick: the post carries the chart once; a refresh edit
/// re-renders the card without uploading (the posted image stays).
#[tokio::test]
async fn the_tick_posts_the_chart_and_refresh_never_uploads() {
    let art = art_dir();
    let store = Arc::new(SqliteStore::open_temp().await);
    let world = world();
    let fake = Arc::new(support::fake());
    let (star, kalos_run) = seed_day_of(&*store).await;
    let cards = CardKit {
        estimates: Some(Arc::new(Fixed(vec![
            (star.clone(), estimate(carling())),
            (kalos_run.clone(), estimate(kalos())),
        ]))),
        ..kit(Some(&art))
    };
    let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(cards.clone());
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let posts = created(&fake);
    let [post] = posts.as_slice() else {
        panic!("one post: {posts:?}");
    };
    let names: Vec<&str> = post
        .attachments
        .iter()
        .map(|upload| upload.filename.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "MaleficStar.png",
            "image-MaleficStar.png",
            "Kalos.webp",
            "estimate.png"
        ],
        "the art first, the chart a plain file no embed names"
    );
    assert!(post.embeds.iter().all(|embed| {
        serde_json::to_string(embed)
            .unwrap()
            .find("estimate.png")
            .is_none()
    }));

    answer(&*store, &star, "1002", true, now() + TimeDelta::minutes(1)).await;
    let refresh = refresher(&store, &fake, &world, cards);
    assert_eq!(refresh.refresh(std::slice::from_ref(&star)).await, 1);
    assert_eq!(edits(&fake).len(), 1, "edited in place");
    assert_eq!(created(&fake).len(), 1, "nothing posted again");
}
