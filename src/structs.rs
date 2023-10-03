use serde::{Deserialize, Serialize};

use sqlx::FromRow;

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

#[derive(Deserialize, Clone)]
pub enum FirstMessageType {
    NewUUID,
    ExistingUUID,
    ChatAgent,
}

#[derive(Deserialize, Clone)]
pub struct FirstMessage {
    pub message_type: FirstMessageType,
    pub message_content: String,
}

#[derive(Serialize, Deserialize, FromRow)]
pub struct Account {
    pub id: i32,
    pub email: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub role: AccountRole,
    pub company_id: i32,
    pub created: chrono::NaiveDateTime,
}

#[derive(Serialize, Deserialize)]
pub struct PublicAccountData {
    // to user only lmao
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub role: AccountRole,
    pub company_id: i32,
    pub created: chrono::NaiveDateTime,
}

#[derive(Serialize, Deserialize)]
pub struct AccountResponse {
    account: PublicAccountData,
    token: String,
}

#[derive(sqlx::Type, Debug, Serialize, Deserialize, Clone, Copy)]
#[sqlx(type_name = "account_role", rename_all = "lowercase")]
pub enum AccountRole {
    Agent = 0,
    Manager = 1,
    Superuser = 2,
}
