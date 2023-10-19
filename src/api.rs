use crate::structs::{Account, AccountResponse, Chat, PublicAccountData, RegisterData};
use crate::utils::{generate_jwt, register_user};
use axum::http::StatusCode;
use axum::{response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

    return (
        StatusCode::OK,
        Json(json!(AccountResponse {
            account: public_account_data,
            token: token,
        })),
    );
}

pub async fn register_handler(
    Extension(pool): Extension<sqlx::PgPool>,
    Json(payload): Json<RegisterData>,
) -> impl IntoResponse {
    let registration_result = register_user(&payload, &pool).await;

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

    let public_account_data = PublicAccountData::from(&db_user.into());
    let token = generate_jwt(&public_account_data).unwrap();

    return (
        StatusCode::OK,
        Json(json!(AccountResponse {
            account: public_account_data,
            token
        })),
    );
}

pub async fn chat_history(Extension(pool): Extension<sqlx::PgPool>) -> impl IntoResponse {
    let chats = sqlx::query_as!(Chat, "SELECT * FROM chat")
        .fetch_all(&pool)
        .await
        .unwrap();

    return (StatusCode::OK, Json(json!(chats)));
}
