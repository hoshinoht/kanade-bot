//! Who carries a boss: each party member's part of the party's combined
//! scouter estimate, read from their own effective % only (never a card's
//! whole-party figure). The party total is the sum of the members'
//! effective %, complete only when every party member has one; a member
//! without one has no part, never 0%. Each boss's parts are that boss's
//! own party: a member may bring another character to a later boss
//! (per-boss picks), and keeps their number and colour.

/// The character a member brings to one boss when it is not their usual
/// one ([`Carrier::ign`]).
#[derive(Clone, Debug, PartialEq)]
pub struct BossCarrier {
    pub ign: String,
    /// The scouter assumed a 100% critical rate for this character.
    pub crit_assumed: bool,
}

/// A party member who brings a character the scouter can look up; their
/// place among the carriers is their number and colour in the chart.
#[derive(Clone, Debug, PartialEq)]
pub struct Carrier {
    /// The roster IGN the estimate was read under: the first boss's
    /// character (else the first boss's they bring one to).
    pub ign: String,
    /// The scouter assumed a 100% critical rate for them.
    pub crit_assumed: bool,
    /// Their cached effective % for each of the party's bosses (of the
    /// character brought to it); `None` without one (nothing cached, or no
    /// figure for that boss).
    pub effective: Vec<Option<f64>>,
    /// Per boss, the character brought when it is not `ign`'s; empty when
    /// they bring `ign` to every boss.
    pub per_boss: Vec<Option<BossCarrier>>,
}

impl Carrier {
    /// A figure for at least one boss.
    pub fn estimated(&self) -> bool {
        self.effective.iter().any(|value| value.is_some_and(usable))
    }

    fn at(&self, boss: usize) -> Option<&BossCarrier> {
        self.per_boss.get(boss).and_then(Option::as_ref)
    }

    /// The IGN of the character brought to boss `boss`.
    pub fn ign_at(&self, boss: usize) -> &str {
        self.at(boss).map_or(&self.ign, |other| &other.ign)
    }

    /// The scouter assumed the critical rate of the character brought to
    /// boss `boss`.
    pub fn crit_at(&self, boss: usize) -> bool {
        self.at(boss)
            .map_or(self.crit_assumed, |other| other.crit_assumed)
    }

    /// An assumed critical rate behind some figure drawn.
    pub fn crit_shown(&self) -> bool {
        self.effective
            .iter()
            .enumerate()
            .any(|(boss, value)| value.is_some_and(usable) && self.crit_at(boss))
    }
}

/// A run's party as the share chart reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyShares {
    /// Catalog boss tokens, in the run's order.
    pub bosses: Vec<String>,
    /// Party members with a character to look up, in party order.
    pub carriers: Vec<Carrier>,
    /// Party members with nothing to look up for any boss (no character,
    /// or one the scouter can't take).
    pub missing: usize,
}

impl PartyShares {
    fn values(&self, boss: usize) -> Vec<Option<f64>> {
        self.carriers
            .iter()
            .map(|carrier| {
                carrier
                    .effective
                    .get(boss)
                    .copied()
                    .flatten()
                    .filter(|value| usable(*value))
            })
            .collect()
    }

    /// The sum of the members' effective % for boss `boss`; `None` when
    /// nobody has one.
    pub fn total(&self, boss: usize) -> Option<f64> {
        let known: Vec<f64> = self.values(boss).into_iter().flatten().collect();
        (!known.is_empty()).then(|| known.iter().sum())
    }

    /// Every party member has an effective % for boss `boss`.
    pub fn complete(&self, boss: usize) -> bool {
        self.missing == 0 && self.values(boss).iter().all(Option::is_some)
    }

    /// Each carrier's share of boss `boss` in whole %, in carrier order
    /// (see [`split`]).
    pub fn shares(&self, boss: usize) -> Vec<Option<u32>> {
        split(&self.values(boss))
    }

    /// Party members with no figure for any boss: counted, never drawn.
    pub fn no_estimate(&self) -> usize {
        self.missing
            + self
                .carriers
                .iter()
                .filter(|carrier| !carrier.estimated())
                .count()
    }

    /// Party members with no figure for boss `boss` (counted, never 0%).
    pub fn no_estimate_at(&self, boss: usize) -> usize {
        self.missing
            + self
                .values(boss)
                .iter()
                .filter(|value| value.is_none())
                .count()
    }

    /// Some figure for boss `boss` comes from an assumed critical rate.
    pub fn crit_at(&self, boss: usize) -> bool {
        self.carriers
            .iter()
            .zip(self.values(boss))
            .any(|(carrier, value)| value.is_some() && carrier.crit_at(boss))
    }

    /// Some member brings another character to some boss: each bar names
    /// its own characters.
    pub fn per_boss(&self) -> bool {
        self.carriers
            .iter()
            .any(|carrier| carrier.per_boss.iter().any(Option::is_some))
    }
}

fn usable(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

/// Whole-% shares of the known values' sum, adding up to exactly 100 by
/// largest remainder (ties to the earlier value); `None` stays `None`.
/// All `None` when the known values sum to nothing.
pub fn split(values: &[Option<f64>]) -> Vec<Option<u32>> {
    let known = |value: &Option<f64>| value.filter(|v| usable(*v));
    let total: f64 = values.iter().filter_map(known).sum();
    if total <= 0.0 {
        return vec![None; values.len()];
    }
    let raw: Vec<Option<f64>> = values
        .iter()
        .map(|value| known(value).map(|v| v / total * 100.0))
        .collect();
    // Each share is within 0..=100, so the cast cannot truncate.
    let mut out: Vec<Option<u32>> = raw
        .iter()
        .map(|share| share.map(|share| share.floor() as u32))
        .collect();
    let given: u32 = out.iter().flatten().sum();
    let mut order: Vec<(f64, usize)> = raw
        .iter()
        .enumerate()
        .filter_map(|(index, share)| share.map(|share| (share - share.floor(), index)))
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, index) in order
        .into_iter()
        .take(100_u32.saturating_sub(given) as usize)
    {
        if let Some(share) = &mut out[index] {
            *share += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carrier(ign: &str, effective: &[Option<f64>]) -> Carrier {
        Carrier {
            ign: ign.into(),
            crit_assumed: false,
            effective: effective.to_vec(),
            per_boss: Vec::new(),
        }
    }

    #[test]
    fn shares_add_up_to_100_over_the_known_members() {
        // The approved board's Kalos numbers.
        let shares = split(&[38.8, 35.8, 25.3, 23.7, 16.8].map(Some));
        assert_eq!(shares, [28, 25, 18, 17, 12].map(Some));
        assert_eq!(shares.iter().flatten().sum::<u32>(), 100);
        // Thirds: the spare point goes to the earliest.
        assert_eq!(split(&[Some(1.0); 3]), [34, 33, 33].map(Some));
    }

    #[test]
    fn a_member_without_a_figure_has_no_share_never_zero() {
        assert_eq!(
            split(&[Some(30.0), None, Some(10.0)]),
            vec![Some(75), None, Some(25)]
        );
        assert_eq!(split(&[None, None]), vec![None, None]);
        assert_eq!(split(&[Some(0.0)]), vec![None], "nothing to share");
        assert_eq!(split(&[Some(f64::NAN), Some(5.0)]), vec![None, Some(100)]);
    }

    #[test]
    fn the_total_is_the_sum_and_complete_needs_everyone() {
        let party = PartyShares {
            bosses: vec!["HCarling".into(), "HMaleficStar".into()],
            carriers: vec![
                carrier("AsahiDK", &[Some(34.0), Some(41.0)]),
                carrier("RenShade", &[Some(22.5), None]),
                carrier("MikaBish", &[None, None]),
            ],
            missing: 0,
        };
        assert_eq!(party.total(0), Some(56.5));
        assert!(!party.complete(0), "MikaBish has nothing cached");
        assert_eq!(party.total(1), Some(41.0));
        assert_eq!(party.shares(1), vec![Some(100), None, None]);
        assert_eq!(party.no_estimate(), 1);
        let cached = PartyShares {
            carriers: party.carriers[..2].to_vec(),
            ..party.clone()
        };
        assert!(cached.complete(0));
        assert!(!cached.complete(1), "a member without a figure");
        let short = PartyShares {
            missing: 2,
            ..party.clone()
        };
        assert!(!short.complete(0), "members with no estimate");
        assert_eq!(short.no_estimate(), 3);
        assert_eq!(party.total(2), None, "no such boss");
    }

    #[test]
    fn a_member_with_another_character_for_a_boss_keeps_their_place() {
        let mut asahi = carrier("AsahiDK", &[Some(34.0), Some(12.0)]);
        asahi.per_boss = vec![
            None,
            Some(BossCarrier {
                ign: "AsahiMech".into(),
                crit_assumed: true,
            }),
        ];
        let party = PartyShares {
            bosses: vec!["HCarling".into(), "HMaleficStar".into()],
            carriers: vec![asahi, carrier("RenShade", &[Some(22.0), Some(36.0)])],
            missing: 0,
        };
        assert!(party.per_boss());
        let asahi = &party.carriers[0];
        assert_eq!((asahi.ign_at(0), asahi.crit_at(0)), ("AsahiDK", false));
        assert_eq!((asahi.ign_at(1), asahi.crit_at(1)), ("AsahiMech", true));
        assert!(asahi.crit_shown());
        assert_eq!(party.shares(1), vec![Some(25), Some(75)]);
        assert!(!party.carriers[1].crit_shown());
        let same = PartyShares {
            carriers: vec![carrier("AsahiDK", &[Some(1.0), Some(1.0)])],
            ..party
        };
        assert!(!same.per_boss());
    }
}
