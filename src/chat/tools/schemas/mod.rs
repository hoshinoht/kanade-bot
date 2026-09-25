//! The closed, ordered model-visible tool surface (v4 `tools/schemas.py`)
//! plus v5's `request_tools`. Schemas and dispatch registration change
//! together.

mod text;

use serde_json::Value;

/// Every tool the model may call, in v4's canonical order, then v5's
/// `request_tools`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolName {
    GetSchedule,
    GetRun,
    ListBosses,
    GetBossStrategy,
    GetPending,
    ListFixed,
    ProposeMove,
    ProposeAdd,
    ProposeCancel,
    ProposeRemoveFixed,
    ProposeChangeFixed,
    ProposeRsvp,
    RequestTools,
}

impl ToolName {
    /// v4's twelve tools in `tool_names()` order.
    pub const V4: [Self; 12] = [
        Self::GetSchedule,
        Self::GetRun,
        Self::ListBosses,
        Self::GetBossStrategy,
        Self::GetPending,
        Self::ListFixed,
        Self::ProposeMove,
        Self::ProposeAdd,
        Self::ProposeCancel,
        Self::ProposeRemoveFixed,
        Self::ProposeChangeFixed,
        Self::ProposeRsvp,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GetSchedule => "get_schedule",
            Self::GetRun => "get_run",
            Self::ListBosses => "list_bosses",
            Self::GetBossStrategy => "get_boss_strategy",
            Self::GetPending => "get_pending",
            Self::ListFixed => "list_fixed",
            Self::ProposeMove => "propose_move",
            Self::ProposeAdd => "propose_add",
            Self::ProposeCancel => "propose_cancel",
            Self::ProposeRemoveFixed => "propose_remove_fixed",
            Self::ProposeChangeFixed => "propose_change_fixed",
            Self::ProposeRsvp => "propose_rsvp",
            Self::RequestTools => "request_tools",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::V4
            .into_iter()
            .chain([Self::RequestTools])
            .find(|tool| tool.as_str() == name)
    }

    /// A card-posting (`propose_*`) tool.
    pub fn is_write(self) -> bool {
        matches!(
            self,
            Self::ProposeMove
                | Self::ProposeAdd
                | Self::ProposeCancel
                | Self::ProposeRemoveFixed
                | Self::ProposeChangeFixed
                | Self::ProposeRsvp
        )
    }

    /// The compact JSON schema, byte-identical to v4's for the v4 tools.
    pub fn schema_text(self) -> &'static str {
        match self {
            Self::GetSchedule => text::GET_SCHEDULE,
            Self::GetRun => text::GET_RUN,
            Self::ListBosses => text::LIST_BOSSES,
            Self::GetBossStrategy => text::GET_BOSS_STRATEGY,
            Self::GetPending => text::GET_PENDING,
            Self::ListFixed => text::LIST_FIXED,
            Self::ProposeMove => text::PROPOSE_MOVE,
            Self::ProposeAdd => text::PROPOSE_ADD,
            Self::ProposeCancel => text::PROPOSE_CANCEL,
            Self::ProposeRemoveFixed => text::PROPOSE_REMOVE_FIXED,
            Self::ProposeChangeFixed => text::PROPOSE_CHANGE_FIXED,
            Self::ProposeRsvp => text::PROPOSE_RSVP,
            Self::RequestTools => text::REQUEST_TOOLS,
        }
    }

    pub fn schema(self) -> Value {
        serde_json::from_str(self.schema_text()).expect("tool schemas are valid JSON")
    }
}

/// `tools` as one compact JSON array, in the order given.
pub fn surface_text(tools: &[ToolName]) -> String {
    let parts: Vec<&str> = tools.iter().map(|tool| tool.schema_text()).collect();
    format!("[{}]", parts.join(","))
}

/// v4's surface: all twelve, or the six read tools on a read-only turn.
pub fn v4_surface(read_only: bool) -> Vec<ToolName> {
    ToolName::V4
        .into_iter()
        .filter(|tool| !(read_only && tool.is_write()))
        .collect()
}

/// v4 `tool_names()`, for the unknown-tool note.
pub fn v4_names() -> Vec<&'static str> {
    ToolName::V4.iter().map(|tool| tool.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_schema_names_its_tool_and_parses() {
        for tool in ToolName::V4.into_iter().chain([ToolName::RequestTools]) {
            let schema = tool.schema();
            assert_eq!(schema["function"]["name"], tool.as_str());
            assert_eq!(ToolName::parse(tool.as_str()), Some(tool));
        }
        assert_eq!(ToolName::parse("delete_run"), None);
    }
}
