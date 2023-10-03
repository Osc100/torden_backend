use crate::structs::{Account, PublicAccountData};
use axum::http::StatusCode;
use axum::{response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

fn get_secret_key() -> String {
    std::env::var("SECRET_KEY").unwrap_or(
        "wvdcrRaOonp0j3YBUErNsbL7iKCNKmHsogHj1wH0gyk4e1VSosoLFr3eLgXCCjhs
NppDoepf5Y0l7mDuTUp0dw=="
            .to_string(),
    )
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LoginRequest {
    email: String,
    password: String,
}

pub async fn login(
    Json(payload): Json<LoginRequest>,
    Extension(pool): Extension<sqlx::PgPool>,
) -> impl IntoResponse {
    let Ok(user) = sqlx::query_as!(
        Account,
        r#"SELECT id, email, first_name, last_name, password, role as "role: _", company_id, created FROM account WHERE email = $1"#,
        payload.email
    )
    .fetch_one(&pool).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "non_field_error": "Invalid email or password"
            })),
        )
    };

    return (StatusCode::OK, Json(json!(user)));
}

pub async fn register(
    Json(payload): Json<Account>,
    Extension(pool): Extension<sqlx::PgPool>,
) -> impl IntoResponse {
    let existing_user = sqlx::query!(r#"SELECT id FROM account WHERE email = $1"#, payload.email)
        .fetch_optional(&pool)
        .await
        .unwrap();

    if existing_user.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "email": "Email already exists"
            })),
        );
    }

    let secret_key = get_secret_key();
    let password_hash = bcrypt::hash(&payload.password, 12).unwrap();

    return (StatusCode::OK, Json(json!({})));
}
