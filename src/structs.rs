use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use sqlx::FromRow;
use sqlx::{types::Uuid, Pool, Postgres};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::{broadcast, Mutex};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIRole {
    System,
    User,
    Assistant,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OpenAIMessage {
    pub role: OpenAIRole,
    pub content: String,
}

impl From<OpenAIMessage> for GPTMessage {
    fn from(value: OpenAIMessage) -> Self {
        Self {
            account_id: None,
            role: MessageRole::Assistant,
            content: value.content.clone(),
        }
    }
}

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
    pub message: OpenAIMessage,
    pub finish_reason: String,
    pub index: u8,
}

#[derive(sqlx::Type, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[sqlx(type_name = "message_role", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// Client messages
    User,
    /// GPT system messages
    System,
    /// Status messages like client or agent connections
    Status,
    /// Messages from the AI
    Assistant,
    /// Messages from the chat agent
    Agent,
}

impl Into<OpenAIRole> for MessageRole {
    fn into(self) -> OpenAIRole {
        match self {
            MessageRole::Agent => OpenAIRole::Assistant,
            MessageRole::Status => OpenAIRole::System,
            MessageRole::Assistant => OpenAIRole::Assistant,
            MessageRole::System => OpenAIRole::System,
            MessageRole::User => OpenAIRole::User,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GPTMessage {
    pub account_id: Option<i32>,
    pub role: MessageRole,
    pub content: String,
}

impl Into<OpenAIMessage> for &GPTMessage {
    fn into(self) -> OpenAIMessage {
        OpenAIMessage {
            role: self.role.into(),
            content: self.content.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Chat {
    pub id: Uuid,
    pub created: chrono::NaiveDateTim,
    pub company_id: i32,
    // pub agents: Option<Vec<GPTMessage>>,
    pub ai_description: Option<String>,
    pub client_name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentPublicData {
    pub id: i32,
    pub first_name: String,
    pub last_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WSMessage {
    pub channel: Uuid,
    pub message: GPTMessage,
    pub agent_data: Option<AgentPublicData>,
}

#[derive(Serialize)]
pub struct DBMessage {
    pub id: i32,
    pub chat_id: Uuid,
    pub role: MessageRole,
    pub text: String,
    pub created: NaiveDateTime,
    pub account_id: Option<i32>,
}

#[derive(Serialize, Deserialize)]
pub struct GPTRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub max_tokens: u16,
}

#[derive(Deserialize)]
pub struct SocketConnectionParams {
    pub client_name: Option<String>,
}

#[derive(Deserialize, Clone, PartialEq, Copy)]
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

#[derive(Serialize, Deserialize, FromRow, Debug)]
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
pub struct RegisterData {
    // Only to be used in restricted registrations, because of the modifiable role.
    pub email: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub role: AccountRole,
    pub company_id: i32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PublicAccountData {
    pub id: i32,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub role: AccountRole,
    pub company_id: i32,
    pub created: chrono::NaiveDateTime,
    pub exp: usize,
}

impl From<&Account> for PublicAccountData {
    fn from(account: &Account) -> Self {
        PublicAccountData {
            id: account.id,
            email: account.email.clone(),
            first_name: account.first_name.clone(),
            last_name: account.last_name.clone(),
            role: account.role,
            company_id: account.company_id,
            created: account.created,
            exp: 1000000000000000,
        }
    }
}

impl From<&Account> for AgentPublicData {
    fn from(value: &Account) -> Self {
        Self {
            id: value.id,
            first_name: value.first_name.clone(),
            last_name: value.last_name.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct AccountResponse {
    pub account: PublicAccountData,
    pub token: String,
}

#[derive(sqlx::Type, Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "account_role", rename_all = "lowercase")]
pub enum AccountRole {
    Agent = 0,
    Manager = 1,
    Superuser = 2,
}

#[derive(PartialEq, Hash, Eq)]
pub struct Company(pub i32);

#[derive(Serialize, Deserialize, Debug)]
pub struct DBCompany {
    pub id: i32,
    pub name: String,
}

#[derive(Clone)]
pub struct AppState {
    pub channels: Arc<Mutex<HashMap<Uuid, ChannelState>>>,
    /// Company ID -> Vec<PublicAccountData>
    pub agent_pool:
        Arc<Mutex<HashMap<Company, Vec<(PublicAccountData, mpsc::Sender<SingleAgentAction>)>>>>,
}

pub type ChannelTransmitter = broadcast::Sender<WSMessage>;

/// Actions in the entire agent pool.
pub enum AgentPoolAction {
    AddAgent((PublicAccountData, mpsc::Sender<SingleAgentAction>)),
    RemoveAgent(PublicAccountData),
    NewChat(Company, Uuid),
    DropChat(Uuid),
}

/// Actions for a single agent
pub enum SingleAgentAction {
    AddChat((Uuid, ChannelState)),
    RemoveChat((Uuid, ChannelTransmitter)),
}
#[derive(Clone)]
pub struct ChannelState {
    pub messages: Vec<GPTMessage>,
    pub company_id: i32,
    pub current_agent: Option<PublicAccountData>,
    pub stopped_ai: bool,
    pub transmitter: ChannelTransmitter,
}

impl ChannelState {
    pub async fn from_db(pool: Pool<Postgres>, uuid: Uuid) -> Self {
        let messages = sqlx::query_as!(
            GPTMessage,
            r#"SELECT account_id, role as "role: MessageRole", text as content FROM message WHERE chat_id = $1"#,
            uuid
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        let company_id = sqlx::query!(r#"SELECT company_id FROM chat WHERE id = $1"#, uuid)
            .fetch_one(&pool)
            .await
            .unwrap()
            .company_id;

        Self {
            messages,
            company_id,
            current_agent: None,
            stopped_ai: false,
            transmitter: broadcast::channel(10).0,
        }
    }

    pub async fn save_message(
        &mut self,
        ws_message: WSMessage,
        pool: &Pool<Postgres>,
    ) -> Result<(), sqlx::Error> {
        let account_id = match ws_message.agent_data {
            Some(agent_data) => Some(agent_data.id),
            None => None,
        };

        tracing::info!("Saving message: {:?}", account_id);

        sqlx::query!(
            "INSERT INTO message (chat_id, role, text, account_id) VALUES ($1, $2, $3, $4)",
            ws_message.channel,
            ws_message.message.role as MessageRole,
            ws_message.message.content,
            account_id
        )
        .execute(pool)
        .await?;

        self.messages.push(ws_message.message);
        Ok(())
    }

    pub async fn send_and_save_message(
        &mut self,
        ws_message: WSMessage,
        pool: &Pool<Postgres>,
    ) -> Result<(), sqlx::Error> {
        self.transmitter.send(ws_message.clone()).unwrap();
        self.save_message(ws_message, pool).await?;

        Ok(())
    }

    pub async fn message_vec_to_ws_messages(
        &self,
        pool: &Pool<Postgres>,
        channel: Uuid,
    ) -> Result<Vec<WSMessage>, sqlx::Error> {
        let mut ws_messages = Vec::new();
        let agents = sqlx::query!(
            r#"SELECT id, first_name, last_name FROM account WHERE company_id = $1 AND role = $2"#,
            self.company_id,
            AccountRole::Agent as _
        )
        .fetch_all(pool)
        .await
        .unwrap();

        for message in &self.messages {
            let agent_data: Option<AgentPublicData> = match message.account_id {
                Some(account_id) => {
                    agents
                        .iter()
                        .find(|&x| x.id == account_id)
                        .map(|x| AgentPublicData {
                            id: x.id,
                            first_name: x.first_name.clone(),
                            last_name: x.last_name.clone(),
                        })
                }
                None => None,
            };

            ws_messages.push(WSMessage {
                channel,
                message: message.to_owned(),
                agent_data,
            })
        }

        Ok(ws_messages)
    }
}
