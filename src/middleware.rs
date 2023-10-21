use crate::utils::decode_jwt;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;

use crate::structs::PublicAccountData;
// Authorized the signature of the JWT.
pub struct Auth(pub PublicAccountData);

#[axum::async_trait]
impl<S> FromRequestParts<S> for Auth
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(user_agent) = parts.headers.get("Authorization") {
            if let Ok(token) = user_agent.to_str() {
                let token = &token.replace("Bearer ", "");

                match decode_jwt(token) {
                    Ok(token) => return Ok(Auth(token.claims)),
                    Err(err) => {
                        tracing::info!("Error with JWT {}", err.to_string());

                        return Err((StatusCode::UNAUTHORIZED, err.to_string()));
                    }
                }
            }
        }

        Err((
            StatusCode::BAD_REQUEST,
            "Missing Authorization header.".to_string(),
        ))
    }
}
