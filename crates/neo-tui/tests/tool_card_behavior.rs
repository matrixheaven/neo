//! Tool card rendering behavior: cards, write/edit batches, approval,
//! shell cards, and consecutive-tool grouping.

#[path = "tool_card_behavior/approval.rs"]
mod approval;
#[path = "tool_card_behavior/cards.rs"]
mod cards;
#[path = "tool_card_behavior/cards_edit.rs"]
mod cards_edit;
#[path = "tool_card_behavior/cards_write.rs"]
mod cards_write;
#[path = "tool_card_behavior/grouping.rs"]
mod grouping;
#[path = "tool_card_behavior/shell.rs"]
mod shell;
