use std::collections::HashMap;

use chrono::{NaiveTime, Utc, Weekday};
use kanade::domain::schedule::Run;
use kanade::extract::plan::{
    BurstInputs, BurstMessage, Payload, Plan, Planned, consolidate, plan_burst,
};
use kanade::extract::schema::parse_response;
use serde_json::{Value, json};

use crate::support::{
    Outcome, amendment_json, catalog, flag, instant, replay_family, resolved_json, runs, select,
    strings, text, unknown_op, zone,
};

fn payload_json(payload: &Payload) -> Value {
    match payload {
        Payload::Empty => json!({}),
        Payload::Fix { weekday, time } => json!({
            "weekday": weekday.num_days_from_monday(),
            "time": time.format_hhmm(),
        }),
        Payload::Split {
            bosses,
            participants,
        } => json!({ "bosses": bosses, "participants": participants }),
        Payload::Sub { remove, add } => json!({ "remove": remove, "add": add }),
    }
}

trait Hhmm {
    fn format_hhmm(&self) -> String;
}

impl Hhmm for chrono::NaiveTime {
    fn format_hhmm(&self) -> String {
        use chrono::Timelike;
        format!("{:02}:{:02}", self.hour(), self.minute())
    }
}

fn planned_json(entry: &Planned<'_>) -> Value {
    json!({
        "kind": entry.kind().as_str(),
        "amendment": amendment_json(&entry.amendment),
        "resolved": resolved_json(&entry.resolved),
        "run_id": entry.run.map(|run| run.id.as_str()),
        "payload": payload_json(&entry.payload),
        "match_reason": entry.match_reason,
        "match_code": entry.match_code,
        "also_mentioned": entry.also_mentioned.iter().map(|kind| kind.as_str()).collect::<Vec<_>>(),
        "ambiguous": entry.ambiguous,
        "summary": entry.summary,
        "needs_answer": entry.needs_answer(),
    })
}

fn plan_json(plan: &Plan<'_>) -> Value {
    json!({
        "planned": plan.planned.iter().map(planned_json).collect::<Vec<_>>(),
        "dropped": plan.dropped.iter().map(planned_json).collect::<Vec<_>>(),
        "summary": plan.summary,
    })
}

/// One burst's plan from its raw scripted model response.
fn planned<'a>(input: &Value, step: &Value, pool: &'a [Run]) -> Plan<'a> {
    let extraction = parse_response(text(&step["raw"])).expect("scripted response accepted");
    let table = catalog(&input["catalog"]);
    let channel = select(pool, &step["channel_runs"]);
    let guild = select(pool, &step["guild_runs"]);
    let order = strings(&step["burst_order"]);
    let authors: HashMap<String, String> = step["author_ids"]
        .as_object()
        .expect("author_ids")
        .iter()
        .map(|(id, author)| (id.clone(), text(author).to_owned()))
        .collect();
    let messages: Vec<BurstMessage> = step["burst_messages"]
        .as_array()
        .expect("burst_messages")
        .iter()
        .map(|message| BurstMessage {
            id: text(&message["id"]).to_owned(),
            author_id: text(&message["author_id"]).to_owned(),
            content: text(&message["content"]).to_owned(),
        })
        .collect();
    let inputs = BurstInputs {
        anchor: instant(&step["anchor"]).with_timezone(&Utc),
        now: instant(&step["now"]).with_timezone(&Utc),
        zone: zone(input),
        reset_weekday: Weekday::Thu,
        reset_time: NaiveTime::MIN,
        channel_runs: &channel,
        guild_runs: &guild,
        burst_order: &order,
        author_ids: &authors,
        // Frozen planning inputs supply one burst anchor, not per-message times.
        message_times: &HashMap::new(),
        min_confidence: step["min_confidence"].as_f64().expect("min_confidence"),
        boss_table: flag(&step["use_boss_table"]).then_some(&table),
        burst_messages: &messages,
    };
    plan_burst(&extraction, &inputs).expect("in range")
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let pool = runs(&input["runs"]);
    let value = match text(&step["op"]) {
        "plan_burst" => plan_json(&planned(input, step, &pool)),
        "consolidate" => {
            let plans: Vec<Plan<'_>> = step["bursts"]
                .as_array()
                .expect("bursts")
                .iter()
                .map(|burst| planned(input, burst, &pool))
                .collect();
            let entries = plans.iter().flat_map(|plan| plan.planned.clone()).collect();
            let consolidated = consolidate(entries);
            json!({
                "plans": plans.iter().map(plan_json).collect::<Vec<_>>(),
                "consolidated": consolidated.iter().map(planned_json).collect::<Vec<_>>(),
            })
        }
        other => unknown_op("plan", other),
    };
    Ok(value)
}

#[test]
fn plan_vectors_replay_exactly() {
    assert_eq!(replay_family("plan", replay), (8, 14));
}
