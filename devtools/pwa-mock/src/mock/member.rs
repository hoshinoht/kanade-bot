//! The member portal's reads for the one mock member (`portal::MEMBER_ID`,
//! the seed's Asahi): `MemberWeek` is the admin week as a member sees it
//! (`docs/notes/public-portal-plan.md` § `MemberWeek`), every run in full,
//! with `mine`/`can_edit` for the caller and the admin-only fields left out.
//! Participant ids are the members' snowflake-shaped ids (`portal::snowflake`),
//! so the portal finds "you" by comparing them with the session's member id.

use super::Store;
use super::dto::{Boss, Run, Tally, WeekDay};
use super::portal::{MEMBER_ID, snowflake};
use serde::Serialize;

#[derive(Serialize)]
pub struct MemberParticipant {
    pub id: String,
    pub name: &'static str,
    pub answer: &'static str,
}

#[derive(Serialize)]
pub struct MemberRun {
    pub id: String,
    pub day: u8,
    pub time: Option<String>,
    pub minutes: u32,
    pub status: &'static str,
    pub bosses: Vec<Boss>,
    pub tally: Tally,
    pub participants: Vec<MemberParticipant>,
    pub party: &'static str,
    pub channel: &'static str,
    pub fixed_id: Option<String>,
    pub mine: bool,
    pub can_edit: bool,
}

#[derive(Serialize)]
pub struct MemberWeek {
    pub starts: String,
    pub timezone: &'static str,
    pub reset: &'static str,
    pub days: Vec<WeekDay>,
    pub runs: Vec<MemberRun>,
    pub generated_at: String,
    pub version: u64,
}

/// One admin run as the caller sees it. Both boss weeks the portal reads
/// (this and next) still take member writes, so `can_edit` turns on the
/// caller's place and the run's status only.
pub(super) fn member_run(run: Run) -> MemberRun {
    let participants: Vec<MemberParticipant> = run
        .participants
        .iter()
        .map(|p| MemberParticipant {
            id: snowflake(p.id),
            name: p.name,
            answer: p.answer,
        })
        .collect();
    let mine = participants.iter().any(|p| p.id == MEMBER_ID);
    MemberRun {
        can_edit: mine && !matches!(run.status, "done" | "cancelled"),
        mine,
        id: run.id,
        day: run.day,
        time: run.time,
        minutes: run.minutes,
        status: run.status,
        bosses: run.bosses,
        tally: run.tally,
        participants,
        party: run.party,
        channel: run.channel,
        fixed_id: run.fixed_id,
    }
}

impl Store {
    /// `GET /api/public/week`: the admin week's frame and order, member-shaped.
    pub fn member_week(&self, next: bool) -> MemberWeek {
        let week = self.week(next);
        MemberWeek {
            starts: week.starts,
            timezone: week.timezone,
            reset: week.reset,
            days: week.days,
            runs: week.runs.into_iter().map(member_run).collect(),
            generated_at: week.generated_at,
            version: week.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mock::tests::store;
    use serde_json::{Value, json};

    fn runs(week: &Value) -> Vec<Value> {
        week["runs"].as_array().unwrap().clone()
    }

    #[test]
    fn the_member_week_marks_the_callers_runs_and_drops_admin_fields() {
        let s = store();
        let week = serde_json::to_value(s.member_week(false)).unwrap();
        let admin = serde_json::to_value(s.week(false)).unwrap();
        let ids = |w: &Value| runs(w).iter().map(|r| r["id"].clone()).collect::<Vec<_>>();
        assert_eq!(ids(&week), ids(&admin), "same runs, same order");
        assert_eq!(week["days"], admin["days"]);
        let mut keys: Vec<String> = week["runs"][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "bosses",
                "can_edit",
                "channel",
                "day",
                "fixed_id",
                "id",
                "mine",
                "minutes",
                "participants",
                "party",
                "status",
                "tally",
                "time"
            ]
        );
        let flags = |id: &str| {
            let run = runs(&week).into_iter().find(|r| r["id"] == id).unwrap();
            (run["mine"].clone(), run["can_edit"].clone())
        };
        // On some runs (a done one included), not on others (the own-time one).
        assert_eq!(flags("r-carling"), (json!(true), json!(true)));
        assert_eq!(flags("r-baldrix"), (json!(true), json!(false)));
        assert_eq!(flags("r-bellona"), (json!(false), json!(false)));
        let carling = runs(&week)
            .into_iter()
            .find(|r| r["id"] == "r-carling")
            .unwrap();
        assert_eq!(carling["participants"][0]["id"], "100000000000001001");
        let admin_carling = runs(&admin)
            .into_iter()
            .find(|r| r["id"] == "r-carling")
            .unwrap();
        assert_eq!(carling["channel"], admin_carling["channel"]);
        assert_eq!(carling["party"], admin_carling["party"]);
        let next = serde_json::to_value(s.member_week(true)).unwrap();
        assert!(runs(&next).iter().any(|r| r["mine"] == true));
    }
}
