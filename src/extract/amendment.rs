use crate::domain::schedule::RsvpState;

/// What an amendment changes, in the order the v4 prompt lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AmendmentKind {
    Move,
    Add,
    Cancel,
    Split,
    Otot,
    Sub,
    Rsvp,
    Fix,
}

impl AmendmentKind {
    pub const ALL: [Self; 8] = [
        Self::Move,
        Self::Add,
        Self::Cancel,
        Self::Split,
        Self::Otot,
        Self::Sub,
        Self::Rsvp,
        Self::Fix,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Add => "add",
            Self::Cancel => "cancel",
            Self::Split => "split",
            Self::Otot => "otot",
            Self::Sub => "sub",
            Self::Rsvp => "rsvp",
            Self::Fix => "fix",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
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
