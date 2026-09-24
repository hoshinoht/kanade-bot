use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Delivery {
    pub guild_id: i64,
    pub run_id: String,
    pub active_delivery_id: String,
    pub occurred_at: String,
}

pub fn fixture() -> Vec<Delivery> {
    vec![
        Delivery {
            guild_id: 42,
            run_id: "run-001".into(),
            active_delivery_id: "message-101".into(),
            occurred_at: "2026-09-21T12:00:00Z".into(),
        },
        Delivery {
            guild_id: 42,
            run_id: "run-002".into(),
            active_delivery_id: "message-102".into(),
            occurred_at: "2026-09-21T12:05:00Z".into(),
        },
    ]
}
