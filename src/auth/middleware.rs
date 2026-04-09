use std::sync::Arc;

use axum::{
    extract::{
        Request,
        State,
    },
    http::header,
    middleware::Next,
    response::{
        IntoResponse,
        Response,
    },
};

use super::error::AuthServiceError;
use crate::deploy::DeployService;

pub async fn authorize_request(
    State(_service): State<Arc<DeployService>>,
    request: Request,
    next: Next,
) -> Response {
    let name = match resolve_protected_route(&request) {
        Ok(route) => route,
        Err(err) => return err.into_response(),
    };

    let token = match bearer_token(&request) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };

    if let Err(err) = super::SERVICE.authorize(token, &name) {
        return err.into_response();
    }

    next.run(request).await
}

fn resolve_protected_route(request: &Request) -> Result<String, AuthServiceError> {
    let path = request.uri().path();

    let segments: Vec<_> = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    match segments.as_slice() {
        ["deploy", name] => Ok((*name).to_string()),
        ["deploy", name, "state"] => Ok((*name).to_string()),
        _ => Err(AuthServiceError::InvalidRoute),
    }
}

fn bearer_token(request: &Request) -> Result<&str, AuthServiceError> {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .ok_or(AuthServiceError::InvalidToken)?;

    let value = header
        .to_str()
        .map_err(|_| AuthServiceError::InvalidToken)?;

    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .ok_or(AuthServiceError::InvalidToken)
}
