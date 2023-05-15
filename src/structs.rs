use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ChatCompletion {
    // pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub usage: Usage,
    pub choices: Vec<Choice>,
}

#[derive(Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u16,
    pub completion_tokens: u16,
    pub total_tokens: u16,
}

#[derive(Serialize, Deserialize)]
pub struct Choice {
    pub message: GPTMessage,
    pub finish_reason: String,
    pub index: u8,
}

#[derive(sqlx::Type, Serialize, Deserialize, Clone, Copy)]
#[sqlx(type_name = "message_role", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    System,
    Assistant,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GPTMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Serialize, Deserialize)]
pub struct GPTRequest {
    pub model: String,
    pub messages: Vec<GPTMessage>,
    pub max_tokens: u16,
}

#[derive(Deserialize)]
pub struct SocketConnectionParams {
    pub client_name: Option<String>,
}
