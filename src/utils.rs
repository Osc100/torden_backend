use axum::Json;
use jsonwebtoken::{errors::Error, TokenData};
use reqwest::Client;
use sqlx::PgPool;

use crate::structs::{
    Account, ChatCompletion, GPTMessage, GPTRequest, MessageRole, PublicAccountData, RegisterData,
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

    let starter_message = GPTMessage {
        role: MessageRole::System,
        content: r#"
        You're a helpful sales representative who works in torden, an startup dedicated to automate customer service using LLMs. 
        Write short, helpful messages only about torden and politely decline questions about anything else.
        Here's some more context that you will answer only if asked: 
        The founders/members are Oscar Marin as Tech Lead and Software Architect, Kelly as Marketing Specialist, Agner as Design Specialist and Katherine as relationship management and sales.
        The startup doesn't have a fisical location yet and will make it's official annoucement in the Hackathon Nicaragua 2023"#.to_string(),
    };

    let request_messages = if conversation_messages
        .first()
        .is_some_and(|x| x.role == MessageRole::System)
    {
        conversation_messages
    } else {
        vec![vec![starter_message], conversation_messages].concat()
    };

    let request_data = GPTRequest {
        model: "gpt-3.5-turbo".to_string(),
        messages: request_messages,
        max_tokens: 250,
    };

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(&std::env::var("OPENAI_API_KEY").unwrap())
        .json(&request_data)
        .send()
        .await
        .unwrap();

    Json(response.json::<ChatCompletion>().await.unwrap())
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
