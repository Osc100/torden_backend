use crate::middleware::Auth;
use crate::structs::{
    Account, AccountResponse, AccountRole, Chat, DBCompany, MessageRole, PublicAccountData,
    RegisterData,
};
use crate::utils::{generate_jwt, register_user};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::{response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug)]
pub struct LoginRequest {
    email: String,
    password: String,
}

pub async fn login_handler(
    Extension(pool): Extension<sqlx::PgPool>,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    let Ok(user) = sqlx::query_as!(
        Account,
        r#"SELECT id, email, first_name, last_name, password, role as "role: _", company_id, created FROM account WHERE email = $1"#,
        payload.email
    )
    .fetch_one(&pool).await else { return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "non_field_error": "Invalid email or password"
            })),
        )
    };

    let password_matches = bcrypt::verify(&payload.password, &user.password).unwrap();

    if !password_matches {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "non_field_error": "Invalid email or password"
            })),
        );
    }

    let public_account_data = PublicAccountData::from(&user);
    let token = generate_jwt(&public_account_data).unwrap();

    (
        StatusCode::OK,
        Json(json!(AccountResponse {
            account: public_account_data,
            token,
        })),
    )
}

pub async fn register_handler(
    Auth(user): Auth,
    Extension(pool): Extension<sqlx::PgPool>,
    Json(payload): Json<RegisterData>,
) -> impl IntoResponse {
    let cheeky_response = (
        StatusCode::BAD_REQUEST,
        Json(json!({
            ":)": "Don't be so smart, you can't do that"
        })),
    );

    let registration_result: Result<Account, String>;

    match user.role {
        AccountRole::Superuser => {
            registration_result = register_user(&payload, &pool).await;
        }
        AccountRole::Manager => {
            if payload.role != AccountRole::Agent || payload.company_id != user.company_id {
                return cheeky_response;
            }
            registration_result = register_user(&payload, &pool).await;
        }
        AccountRole::Agent => {
            return cheeky_response;
        }
    };

    let db_user = match registration_result {
        Ok(user) => user,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "non_field_error": e
                })),
            )
        }
    };

    let public_account_data = PublicAccountData::from(&db_user);
    let token = generate_jwt(&public_account_data).unwrap();

    (
        StatusCode::OK,
        Json(json!(AccountResponse {
            account: public_account_data,
            token
        })),
    )
}

pub async fn chat_history(
    Auth(user): Auth,
    Extension(pool): Extension<sqlx::PgPool>,
) -> impl IntoResponse {
    let chats = match user.role {
        AccountRole::Superuser => {
            sqlx::query_as!(
                Chat,
                "SELECT DISTINCT chat.* FROM chat JOIN message ON chat.id = message.chat_id
                ORDER BY chat.created ASC
                "
            )
            .fetch_all(&pool)
            .await
        }
        AccountRole::Agent => {
            sqlx::query_as!(
                Chat,
                r#"SELECT DISTINCT chat.* 
                FROM chat
                JOIN message ON chat.id = message.chat_id
                WHERE company_id = $1
                ORDER BY chat.created ASC
                "#,
                user.company_id
            )
            .fetch_all(&pool)
            .await
        }
        AccountRole::Manager => {
            sqlx::query_as!(
                Chat,
                r#"SELECT DISTINCT chat.* FROM chat
                JOIN message  
                ON chat.id = message.chat_id
                WHERE company_id = $1 AND message.account_id = $2
                ORDER BY chat.created ASC
                "#,
                user.company_id,
                user.id
            )
            .fetch_all(&pool)
            .await
        }
    };

    let chats = chats.unwrap();

    // let agents = sqlx::query!(
    //     r#"SELECT DISTINCT account.id as "id", first_name, last_name, chat_id FROM account
    //     INNER JOIN message m
    //     ON m.account_id = account.id
    //     INNER JOIN chat
    //     ON chat.id = m.chat_id
    //     WHERE chat.id = any($1)"#,
    //     &chats.iter().map(|x| x.id).collect::<Vec<Uuid>>()
    // ).fetch_all(&pool).await.unwrap();

    (StatusCode::OK, Json(chats))
}

pub async fn list_companies(
    Auth(user): Auth,
    Extension(pool): Extension<sqlx::PgPool>,
) -> impl IntoResponse {
    let companies = match user.role {
        AccountRole::Superuser => {
            sqlx::query_as!(DBCompany, "SELECT * FROM company")
                .fetch_all(&pool)
                .await
        }
        _ => {
            sqlx::query_as!(
                DBCompany,
                "SELECT * FROM company WHERE id = $1",
                user.company_id
            )
            .fetch_all(&pool)
            .await
        }
    };

    let companies = companies.unwrap();

    (StatusCode::OK, Json(companies))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DBUser {
    pub id: i32,
    pub first_name: String,
    pub last_name: String,
    pub company_id: i32,
}

pub async fn list_users(
    Auth(user): Auth,
    Extension(pool): Extension<sqlx::PgPool>,
) -> impl IntoResponse {
    let agents = match user.role {
        AccountRole::Superuser => {
            sqlx::query_as!(
                DBUser,
                r#"SELECT id, first_name, last_name, company_id FROM account"#
            )
            .fetch_all(&pool)
            .await
        }
        _ => sqlx::query_as!(
            DBUser,
            r#"SELECT id, first_name, last_name, company_id FROM account WHERE company_id = $1"#,
            user.company_id
        )
        .fetch_all(&pool)
        .await,
    };

    let agents = agents.unwrap();

    (StatusCode::OK, Json(agents))
}

#[derive(Serialize)]
struct DBMessage {
    role: MessageRole,
    text: String,
    created: chrono::NaiveDateTime,
    account_id: Option<i32>,
    first_name: Option<String>,
    last_name: Option<String>,
}

pub async fn chat_messages(
    Auth(_): Auth,
    Extension(pool): Extension<sqlx::PgPool>,
    Path(chat_uuid): Path<Uuid>,
) -> impl IntoResponse {
    let messages = sqlx::query_as!(
        DBMessage,
        r#"SELECT message.role as "role:_", text, message.created as "created", account_id as "account_id?",
        first_name as "first_name?", last_name as "last_name?"
           FROM message
           LEFT OUTER JOIN
           account ON account.id = message.account_id
           WHERE chat_id = $1
           "#,
        chat_uuid,
        // user.company_id
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    (StatusCode::OK, Json(json!(messages)))
}

// From a reqstruct ExtractUserAgent(HeaderValue);
