//! Fixed-PATCH replay identity and its refusal-boundary recovery.

use std::collections::BTreeMap;

use super::*;

/// The full normalized fixed-PATCH form. It stays separate from the derived
/// edit because that edit intentionally depends on the row at retry time.
pub(super) struct FixedPatchIdentity {
    fixed_id: String,
    weekday: Weekday,
    time: NaiveTime,
    bosses: Vec<String>,
    participants: Vec<String>,
    channel_id: String,
    note: Option<String>,
    owner_id: Option<String>,
    version: Option<u64>,
    decisions: BTreeMap<String, String>,
    expect: Option<Vec<(String, Option<u64>)>>,
    overrides: Option<Vec<(u64, String)>>,
}

impl std::fmt::Debug for FixedPatchIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FixedPatchIdentity")
            .field("fixed_id", &self.fixed_id)
            .field("weekday", &self.weekday)
            .field("time", &self.time)
            .field("bosses", &self.bosses)
            .field("participants", &self.participants)
            .field("channel_id", &self.channel_id)
            .field("note", &self.note)
            .field("owner_id", &self.owner_id)
            .field("version", &self.version)
            .field("decisions", &self.decisions)
            .field("expect", &self.expect)
            .field("overrides", &self.overrides)
            .finish()
    }
}

impl FixedPatchIdentity {
    pub(super) fn new(fixed_id: String, timing: &Checked, request: &FixedRequest) -> Self {
        let expect = request.expect.as_ref().map(|fields| {
            let mut fields: Vec<_> = fields
                .iter()
                .map(|field| (field.field.clone(), field.seen))
                .collect();
            fields.sort();
            fields
        });
        let overrides = request.overrides.as_ref().map(|overrides| {
            let mut overrides: Vec<_> = overrides
                .iter()
                .map(|override_| (override_.seq, override_.hash.clone()))
                .collect();
            overrides.sort();
            overrides
        });
        Self {
            fixed_id,
            weekday: timing.weekday,
            time: timing.time,
            bosses: timing.bosses.clone(),
            participants: timing.participants.clone(),
            channel_id: timing.channel_id.clone(),
            note: timing.note.clone(),
            owner_id: timing.owner_id.clone(),
            version: request.version,
            decisions: request.decisions.clone(),
            expect,
            overrides,
        }
    }
}

/// If a key appeared after the handler's initial lookup, normalise only long
/// enough to check it. A relaxed context can never flow into a fresh write.
pub(super) struct RefusalReplay<'a> {
    pub site: &'a Site,
    pub state: &'a ApiState,
    pub origin: &'a Origin,
    pub fixed_id: &'a str,
    pub request: &'a FixedRequest,
    pub profiles: &'a [MemberProfile],
}

impl RefusalReplay<'_> {
    pub(super) async fn recover(&self, context: &mut WriteContext, refusal: Refusal) -> Reply {
        if self.origin.request_id.is_none() || recorded(self.state, self.origin).await?.is_none() {
            return Err(refusal);
        }
        as_first_seen(context, self.request);
        let Ok(timing) = checked(self.state, self.request, &context.directory) else {
            return Err(refusal);
        };
        let replay = FixedPatchReplay::from_normalized(FixedPatchIdentity::new(
            self.fixed_id.to_owned(),
            &timing,
            self.request,
        ));
        match self
            .state
            .writer
            .verify_fixed_patch_replay(self.origin.clone(), replay)
            .await
        {
            Err(SchedulerError::AlreadyApplied { .. }) => {
                row(self.site, self.state, self.fixed_id, self.profiles).await
            }
            Err(error) => Err(scheduler(error)),
            Ok(()) => Err(refusal),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(
        expect: Option<Vec<SeenField>>,
        overrides: Option<Vec<OverrideRef>>,
    ) -> FixedPatchIdentity {
        FixedPatchIdentity::new(
            "f-kalos".into(),
            &Checked {
                weekday: Weekday::Tue,
                time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
                bosses: vec!["XKalos".into()],
                participants: vec!["1001".into(), "1002".into()],
                channel_id: "kalos-four".into(),
                note: Some("seed".into()),
                owner_id: None,
            },
            &FixedRequest {
                weekday: 1,
                time: "21:30".into(),
                bosses: "xkalos".into(),
                participants: vec!["1001".into(), "1002".into()],
                channel_id: "kalos-four".into(),
                note: Some("seed".into()),
                owner_id: None,
                decisions: BTreeMap::new(),
                version: Some(7),
                expect,
                overrides,
            },
        )
    }

    #[test]
    fn fixed_patch_identity_canonicalizes_order_without_dropping_entries() {
        let first = identity(
            Some(vec![
                SeenField {
                    field: "time".into(),
                    seen: Some(4),
                },
                SeenField {
                    field: "note".into(),
                    seen: None,
                },
            ]),
            Some(vec![
                OverrideRef {
                    seq: 4,
                    hash: "four".into(),
                },
                OverrideRef {
                    seq: 2,
                    hash: "two".into(),
                },
            ]),
        );
        let reordered = identity(
            Some(vec![
                SeenField {
                    field: "note".into(),
                    seen: None,
                },
                SeenField {
                    field: "time".into(),
                    seen: Some(4),
                },
            ]),
            Some(vec![
                OverrideRef {
                    seq: 2,
                    hash: "two".into(),
                },
                OverrideRef {
                    seq: 4,
                    hash: "four".into(),
                },
            ]),
        );
        let duplicate = identity(
            Some(vec![
                SeenField {
                    field: "note".into(),
                    seen: None,
                },
                SeenField {
                    field: "note".into(),
                    seen: None,
                },
                SeenField {
                    field: "time".into(),
                    seen: Some(4),
                },
            ]),
            Some(vec![
                OverrideRef {
                    seq: 2,
                    hash: "two".into(),
                },
                OverrideRef {
                    seq: 4,
                    hash: "four".into(),
                },
            ]),
        );
        assert_eq!(format!("{first:?}"), format!("{reordered:?}"));
        assert_ne!(format!("{first:?}"), format!("{duplicate:?}"));
        assert_ne!(
            format!("{:?}", identity(None, None)),
            format!("{:?}", identity(Some(vec![]), None))
        );
        assert_ne!(
            format!("{:?}", identity(None, None)),
            format!("{:?}", identity(None, Some(vec![])))
        );
    }

    #[test]
    fn replay_normalization_recreates_lost_roster_and_channel_only_in_its_context() {
        let request = FixedRequest {
            weekday: 1,
            time: "21:30".into(),
            bosses: "xkalos".into(),
            participants: vec!["1001".into()],
            channel_id: "lost-channel".into(),
            note: None,
            owner_id: Some("1002".into()),
            decisions: BTreeMap::new(),
            version: Some(7),
            expect: None,
            overrides: None,
        };
        let mut context = WriteContext {
            policy: crate::domain::schedule::SchedulePolicy::new(
                crate::domain::schedule::ReminderPolicy {
                    zone: chrono_tz::Asia::Kuala_Lumpur,
                    ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    countdowns: vec![60, 15],
                },
                Weekday::Thu,
                NaiveTime::MIN,
            ),
            directory: Roster::new(),
        };
        as_first_seen(&mut context, &request);
        assert!(validate_participants(&context.directory, &request.participants).is_ok());
        assert!(validate_channel(&context.directory, &request.channel_id).is_ok());
    }
}
