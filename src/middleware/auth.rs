use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};

#[derive(Debug, Clone)]
pub struct ClientAuth(pub String);

#[async_trait]
impl<S> FromRequestParts<S> for ClientAuth
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(auth_header) = parts.headers.get("Authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                return Ok(ClientAuth(auth_str.to_string()));
            }
        }

        if let Some(api_key_header) = parts.headers.get("x-api-key") {
            if let Ok(api_key_str) = api_key_header.to_str() {
                return Ok(ClientAuth(format!("Bearer {}", api_key_str)));
            }
        }

        Err((
            StatusCode::UNAUTHORIZED,
            "Missing or invalid Authorization / x-api-key header",
        ))
    }
}
