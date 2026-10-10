//! In-memory `RunPromptStore`, mirroring SQLite's `run_prompts`.

use std::collections::BTreeMap;

use crate::domain::completion::{PromptClose, PromptFuture, RunPrompt, RunPromptStore};
use crate::domain::scheduler::StoreError;

/// Asks by `(run id, ask)`.
pub(super) type RunPromptTable = BTreeMap<(String, u32), RunPrompt>;

fn insert(table: &mut RunPromptTable, prompt: RunPrompt) -> Result<(), StoreError> {
    let taken = table
        .values()
        .any(|row| row.run_id == prompt.run_id && (row.ask == prompt.ask || row.is_open()));
    if taken {
        return Err(StoreError::Constraint(
            "the run already has this ask or an open one".into(),
        ));
    }
    table.insert((prompt.run_id.clone(), prompt.ask), prompt);
    Ok(())
}

impl super::MemoryScheduleStore {
    fn prompts(&self) -> std::sync::MutexGuard<'_, RunPromptTable> {
        self.run_prompts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn sorted(&self, keep: impl Fn(&RunPrompt) -> bool) -> Vec<RunPrompt> {
        self.prompts()
            .values()
            .filter(|row| keep(row))
            .cloned()
            .collect()
    }
}

impl RunPromptStore for super::MemoryScheduleStore {
    fn create_run_prompt(&self, prompt: RunPrompt) -> PromptFuture<'_, ()> {
        Box::pin(async move {
            prompt.check_new()?;
            insert(&mut self.prompts(), prompt)
        })
    }

    fn run_prompt(&self, run_id: String, ask: u32) -> PromptFuture<'_, Option<RunPrompt>> {
        Box::pin(async move { Ok(self.prompts().get(&(run_id, ask)).cloned()) })
    }

    fn latest_run_prompt(&self, run_id: String) -> PromptFuture<'_, Option<RunPrompt>> {
        Box::pin(async move {
            Ok(self
                .prompts()
                .values()
                .filter(|row| row.run_id == run_id)
                .max_by_key(|row| row.ask)
                .cloned())
        })
    }

    fn open_run_prompts(&self) -> PromptFuture<'_, Vec<RunPrompt>> {
        Box::pin(async move {
            let mut open = self.sorted(RunPrompt::is_open);
            open.sort_by(|a, b| (a.due_at, &a.run_id).cmp(&(b.due_at, &b.run_id)));
            Ok(open)
        })
    }

    fn close_run_prompt(
        &self,
        run_id: String,
        ask: u32,
        close: PromptClose,
    ) -> PromptFuture<'_, bool> {
        Box::pin(async move {
            close.check(&run_id)?;
            let mut table = self.prompts();
            if !table
                .get(&(run_id.clone(), ask))
                .is_some_and(RunPrompt::is_open)
            {
                return Ok(false);
            }
            // Check the next ask against the table as it will be once closed,
            // so a refused insert leaves the close unapplied too.
            let mut after = table.clone();
            let row = after
                .get_mut(&(run_id, ask))
                .expect("checked present above");
            row.outcome = Some(close.outcome);
            row.decided_by = close.decided_by;
            row.decided_at = Some(close.at);
            if let Some(next) = close.next {
                insert(&mut after, next)?;
            }
            *table = after;
            Ok(true)
        })
    }

    fn set_run_prompt_message(
        &self,
        run_id: String,
        ask: u32,
        channel_id: String,
        message_id: String,
    ) -> PromptFuture<'_, ()> {
        Box::pin(async move {
            if let Some(row) = self.prompts().get_mut(&(run_id, ask)) {
                row.channel_id = Some(channel_id);
                row.message_id = Some(message_id);
            }
            Ok(())
        })
    }

    fn unsettled_run_prompts(&self) -> PromptFuture<'_, Vec<RunPrompt>> {
        Box::pin(async move {
            let mut closed = self
                .sorted(|row| !row.is_open() && row.message_id.is_some() && !row.message_settled);
            closed.sort_by(|a, b| {
                (a.decided_at, &a.run_id, a.ask).cmp(&(b.decided_at, &b.run_id, b.ask))
            });
            Ok(closed)
        })
    }

    fn settle_run_prompt_message(&self, run_id: String, ask: u32) -> PromptFuture<'_, ()> {
        Box::pin(async move {
            if let Some(row) = self.prompts().get_mut(&(run_id, ask)) {
                row.message_settled = true;
            }
            Ok(())
        })
    }
}
