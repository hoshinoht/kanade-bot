use crate::domain::schedule::RsvpState;

crate::domain::spelled! {
    /// What an amendment changes, in the order the v4 prompt lists them.
    AmendmentKind {
        Move => "move",
        Add => "add",
        Cancel => "cancel",
        Split => "split",
        Otot => "otot",
        Sub => "sub",
        Rsvp => "rsvp",
        Fix => "fix",
    }
}

/// One proposed change as the model saw it (v4 `schema.Amendment`), already
/// coerced. Day and time stay literal text; `resolve` pins them down.
#[derive(Clone, Debug, PartialEq)]
pub struct Amendment {
    pub kind: AmendmentKind,
    pub bosses: Vec<String>,
    pub day_ref: Option<String>,
    pub time_ref: Option<String>,
    pub participants: Vec<String>,
    pub rsvp: Option<RsvpState>,
    pub is_question: bool,
    /// 0.0..=1.0.
    pub confidence: f64,
    pub evidence_message_ids: Vec<String>,
    pub target_run_hint: Option<String>,
}

impl Amendment {
    /// An amendment of `kind` with v4's field defaults.
    pub fn new(kind: AmendmentKind) -> Self {
        Self {
            kind,
            bosses: Vec::new(),
            day_ref: None,
            time_ref: None,
            participants: Vec::new(),
            rsvp: None,
            is_question: false,
            confidence: 0.0,
            evidence_message_ids: Vec::new(),
            target_run_hint: None,
        }
    }
}
