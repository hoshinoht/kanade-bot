//! The plain-text test messages, rendered as the real notices are: a move
//! from a day earlier and a decline by the invoker.

use chrono::TimeDelta;

use crate::bot::cards::format::Audience;
use crate::bot::delivery::cards::redesign::{self, NoticeLook};
use crate::bot::delivery::cards::{CardContext, People};
use crate::domain::notify::display_names;
use crate::domain::schedule::{Notice, NoticeChange, Run};

/// The move notice for `run` from a day earlier, naming its party.
pub fn amend_text(ctx: &CardContext<'_>, run: &Run, mentioned: &[String]) -> Option<String> {
    let notice = Notice {
        change: NoticeChange::RunMoved {
            run_id: run.id.clone(),
            from: run.datetime - TimeDelta::days(1),
            to: run.datetime,
        },
        channel_id: run.channel_id.clone(),
        listed: run.participants.clone(),
        via_portal: false,
    };
    let look = NoticeLook {
        schedule: ctx.schedule,
        zone: ctx.zone,
        catalog: ctx.catalog,
        marks: ctx.marks,
        quiet: ctx.quiet,
        portal: ctx.v2.and_then(|kit| kit.portal_url()),
    };
    let who = Audience {
        names: display_names(ctx.members, &run.participants)
            .into_iter()
            .collect(),
        mentioned: mentioned.to_vec(),
    };
    redesign::notice_text(&notice, &look, &who)
}

/// The decline notice as `decliner` (shown as `decliner_name`), telling
/// the rest of the party.
pub fn decline_text(
    ctx: &CardContext<'_>,
    run: &Run,
    mentioned: &[String],
    decliner: &str,
    decliner_name: &str,
) -> String {
    let others: Vec<String> = run
        .participants
        .iter()
        .filter(|user| *user != decliner)
        .cloned()
        .collect();
    let recipients = if others.is_empty() {
        String::new()
    } else {
        People::new(ctx, &others, mentioned).list(&others)
    };
    redesign::decline_text(decliner_name, run, &recipients, ctx.catalog, ctx.marks)
}
