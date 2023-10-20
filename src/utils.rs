use std::sync::Arc;

use axum::Json;
use jsonwebtoken::{errors::Error, TokenData};
use reqwest::Client;
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::structs::{
    Account, AppState, ChatCompletion, Choice, Company, GPTMessage, GPTRequest, MessageRole,
    OpenAIMessage, OpenAIRole, PublicAccountData, RegisterData, SingleAgentAction, Usage,
};

const HASH_SALT: u32 = 12;
/// Tries to register to the DB, returns the user if successful.
/// Email must be unique and is enforced in the db.
pub async fn register_user(
    user_to_register: &RegisterData,
    pool: &PgPool,
) -> Result<Account, String> {
    let password_hash =
        bcrypt::hash(&user_to_register.password, HASH_SALT).map_err(|e| e.to_string())?;

    let db_user = sqlx::query_as!(
        Account,
        r#"INSERT INTO account (email, first_name, last_name, password, role, company_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, email, first_name, last_name, password, role as "role: _", company_id, created"#,
        user_to_register.email,
        user_to_register.first_name,
        user_to_register.last_name,
        password_hash,
        user_to_register.role as _,
        user_to_register.company_id
    ).fetch_one(pool).await.map_err(|e| e.to_string())?;
    // uniqueness is enforced at the database level.

    return Ok(db_user);
}

pub async fn query_to_openai(conversation_messages: Vec<GPTMessage>) -> Json<ChatCompletion> {
    let client = Client::new();

    let starter_message = OpenAIMessage {
        role: OpenAIRole::System,
        content: r#"
        You're a helpful sales representative who works in torden, an startup dedicated to automate customer service using LLMs. 
        Write short, helpful messages only about torden and politely decline questions about anything else.
        Here's some more context that you will answer only if asked: 
        The founders/members are Oscar Marin as Tech Lead and Software Architect, Kelly as Marketing Specialist, Agner as Design Specialist and Katherine as relationship management and sales.
        The startup doesn't have a fisical location yet and will make it's official annoucement in the Hackathon Nicaragua 2023"#.to_string(),
    };

    let dummy_message = ChatCompletion {
        object: "ADW".to_string(),
        created: 0,
        model: "dwalkjd".to_string(),
        choices: vec![Choice {
            message: OpenAIMessage {
                role: OpenAIRole::Assistant,
                content: r#"
                    I'm sorry, I'm not sure I understand. Could you please rephrase your question?
                    I'm sorry, I'm not sure I understand. Could you please rephrase your question?
                    I'm sorry, I'm not sure I understand. Could you please rephrase your question?
                    I'm sorry, I'm not sure I understand. Could you please rephrase your question?"#
                    .to_string(),
            },
            index: 0,
            finish_reason: "None".to_string(),
        }],
        usage: Usage {
            prompt_tokens: 30,
            completion_tokens: 30,
            total_tokens: 60,
        },
    };

    // Filter internal MessageRole to something supported by the OpenAI API.
    let conversation_messages: Vec<OpenAIMessage> = conversation_messages
        .iter()
        .filter(|x| x.role != MessageRole::Status)
        .map(|x| x.into())
        .collect();

    let request_messages = if conversation_messages
        .first()
        .is_some_and(|x| x.role == OpenAIRole::System)
    {
        conversation_messages
    } else {
        vec![vec![starter_message], conversation_messages].concat()
    };

    let request_data = GPTRequest {
        model: "gpt-3.5-turbo".to_string(),
        messages: request_messages
            .iter()
            .map(|x| x.to_owned().into())
            .collect(),
        max_tokens: 250,
    };

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(&std::env::var("OPENAI_API_KEY").unwrap())
        .json(&request_data)
        .send()
        .await;

    match response {
        Ok(response) => {
            tracing::info!("GPT Request {}", response.status());

            Json(
                response
                    .json::<ChatCompletion>()
                    .await
                    .map_err(|e| {
                        tracing::error!("Error with GPT Request {}", e.to_string());
                        e.to_string()
                    })
                    .unwrap_or(dummy_message),
            )
        }
        Err(err) => {
            tracing::error!("Error with GPT Request {}", err.to_string());
            return Json(dummy_message);
        }
    }
}

pub fn generate_jwt(user: &PublicAccountData) -> Result<String, Error> {
    return jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        user,
        &jsonwebtoken::EncodingKey::from_secret(get_secret_key().as_bytes()),
    );
}

pub fn decode_jwt(token: &str) -> Result<TokenData<PublicAccountData>, Error> {
    return jsonwebtoken::decode::<PublicAccountData>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(get_secret_key().as_bytes()),
        &jsonwebtoken::Validation::default(),
    );
}

pub fn get_secret_key() -> String {
    std::env::var("SECRET_KEY").unwrap_or(
        "wvdcrRaOonp0j3YBUErNsbL7iKCNKmHsogHj1wH0gyk4e1VSosoLFr3eLgXCCjhs
NppDoepf5Y0l7mDuTUp0dw=="
            .to_string(),
    )
}

/// Returns a vector of tuples with the agent id and the number of chats they are currently in.
pub async fn get_chats_per_agent(
    app_state: Arc<AppState>,
    company_id: i32,
) -> Vec<(PublicAccountData, mpsc::Sender<SingleAgentAction>, usize)> {
    let channels = app_state.channels.lock().await;
    let agents = app_state.agent_pool.lock().await;

    let Some(company_agents) = agents.get(&Company(company_id)) else {
        return vec![];
    };

    let agent_chats = company_agents
        .iter()
        .map(|agent| {
            (
                agent.0.to_owned(),
                agent.1.to_owned(),
                channels
                    .iter()
                    .filter(|(_, channel)| {
                        channel
                            .current_agent
                            .as_ref()
                            .is_some_and(|current_agent| current_agent.id == agent.0.id)
                    })
                    .count(),
            )
        })
        .collect();

    return agent_chats;
}
