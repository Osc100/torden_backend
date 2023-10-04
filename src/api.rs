use crate::structs::{Account, AccountResponse, PublicAccountData, RegisterData};
use crate::utils::register_user;
use axum::http::StatusCode;
use axum::{response::IntoResponse, Extension, Json};
use jsonwebtoken::errors::Error;
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
    Json(payload): Json<RegisterData>,
    Extension(pool): Extension<sqlx::PgPool>,
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

fn generate_jwt(user: &PublicAccountData) -> Result<String, Error> {
    return jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        user,
        &jsonwebtoken::EncodingKey::from_secret(get_secret_key().as_bytes()),
    );
}
