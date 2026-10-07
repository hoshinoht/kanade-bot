//! The admin fixed-PATCH-only request identity path.

use super::*;

impl<S: ScheduleStore, I: IdSource, C: Clock> Attributed<'_, S, I, C> {
    /// The API's complete fixed-PATCH identity replaces only this operation's
    /// derived edit digest; all existing scheduler callers keep theirs.
    fn fixed_patch_meta(&self, identity: &str) -> ChangeMeta {
        ChangeMeta {
            origin: self.origin.clone(),
            at: DateTime::UNIX_EPOCH,
            notices: Vec::new(),
            refs: self.expect.overrides.clone(),
            request_digest: self
                .origin
                .request_id
                .as_ref()
                .map(|_| digest("fixed_patch.v1", &identity)),
            expect: self.expect.clone(),
            outbox: Vec::new(),
        }
    }

    /// Check a fixed PATCH's recorded request before current-state planning,
    /// precondition validation, or no-op handling.
    pub async fn verify_fixed_patch_replay(self, identity: &str) -> SchedulerResult<()> {
        let meta = self.fixed_patch_meta(identity);
        check_request(&self.service.store, &meta).await
    }

    async fn apply_fixed_patch(
        self,
        scope: Scope,
        op: Op<'_>,
        identity: &str,
    ) -> SchedulerResult<Outcome<OpResult>> {
        let meta = self.fixed_patch_meta(identity);
        // A recorded retry survives authority/precondition changes since its
        // first write. The transaction repeats this check for commit races.
        check_request(&self.service.store, &meta).await?;
        if let Some(policy) = op.schedule_policy() {
            self.service.check_policy(policy)?;
        }
        meta.expect
            .validate(&meta.origin.actor)
            .map_err(SchedulerError::Precondition)?;
        let overriding = !meta.expect.overrides.is_empty();
        let surface = meta.origin.surface;
        let outcome = self
            .service
            .transact_as(
                meta.clone(),
                scope,
                |draft, ids, now| {
                    apply_op(draft, ids, &op, now).map(|outcome| on_surface(&op, surface, outcome))
                },
                |outcome: &Outcome<OpResult>| {
                    let mut kinds: Vec<String> =
                        outcome.notices.iter().map(Notice::effect_kind).collect();
                    if overriding {
                        kinds.push(EDIT_OVERRIDE.to_owned());
                    }
                    (kinds, outcome.notices.clone())
                },
            )
            .await;
        match outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => match check_request(&self.service.store, &meta).await {
                // `transact_as` may discover a no-op or planning refusal after
                // another writer recorded this key. Replay identity still wins.
                Err(replay) => Err(replay),
                Ok(()) => Err(error),
            },
        }
    }

    /// The admin fixed-PATCH variant, whose identity names the complete form
    /// rather than only the state-dependent edit diff.
    pub async fn apply_fixed_patch_edit(
        self,
        request: &FixedEditRequest,
        identity: &str,
        directory: &(impl Directory + Sync),
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<FixedRun>> {
        let op = Op::ApplyFixedEdit {
            request: request.clone(),
            directory,
            policy,
        };
        let outcome = self.apply_fixed_patch(Scope::All, op, identity).await?;
        match outcome.value {
            OpResult::Fixed(fixed) => Ok(Outcome {
                value: fixed,
                notices: outcome.notices,
            }),
            other => unreachable!("apply_fixed_patch_edit returned {other:?}"),
        }
    }
}
