use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegateMailboxMessage {
    pub id: String,
    pub text: String,
    pub delivered: bool,
}
