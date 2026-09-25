//! Stored runtime settings (`extract_model`, `extract_reasoning`,
//! `chat_pilot_model`, `chat_pilot_think`, `v5.rewrite_*`) as setup roles.

use super::super::Effort;
use super::{ModelRoles, RoleEffort, RoleModel};
use crate::domain::settings::{self, Reasoning};

impl From<&settings::Models> for ModelRoles {
    fn from(models: &settings::Models) -> Self {
        Self {
            extraction: role(&models.extraction),
            chat: role(&models.chat),
            rewrite: role(&models.rewrite),
        }
    }
}

fn role(stored: &settings::RoleModel) -> RoleModel {
    RoleModel {
        alias: stored.alias.clone(),
        effort: effort(stored.reasoning),
    }
}

fn effort(reasoning: Reasoning) -> RoleEffort {
    RoleEffort::Level(match reasoning {
        Reasoning::Inherit => return RoleEffort::Inherit,
        Reasoning::Off => Effort::Off,
        Reasoning::Minimal => Effort::Minimal,
        Reasoning::Low => Effort::Low,
        Reasoning::Medium => Effort::Medium,
        Reasoning::High => Effort::High,
        Reasoning::Xhigh => Effort::Xhigh,
        Reasoning::Max => Effort::Max,
    })
}
