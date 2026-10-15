//! The full-set tool surface keeps v4's twelve tools in order.

use kanade::chat::tools::ToolName;
use kanade::chat::tools::bundles::ToolOffer;

#[test]
fn the_full_set_is_v4s_twelve_in_order() {
    let names: Vec<&str> = ToolName::V4.iter().map(|tool| tool.as_str()).collect();
    assert_eq!(ToolOffer::full_set(false).names(), names);
    assert!(!ToolOffer::full_set(false).offers(ToolName::RequestTools));
}
