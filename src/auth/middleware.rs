use axum::{
    extract::Request,
    http::header,
    middleware::Next,
    response::{
        IntoResponse,
        Response,
    },
};

use super::error::AuthServiceError;
use crate::deploy::DeployService;

pub async fn authorize_request(request: Request, next: Next) -> Response {
    let target = match resolve_access_target(&request) {
        Ok(route) => route,
        Err(err) => return err.into_response(),
    };

    let (name, token) = match bearer_credentials(&request) {
        Ok(creds) => creds,
        Err(err) => return err.into_response(),
    };

    if let Err(err) = super::authorize(DeployService::global().base(), name, token, &target) {
        return err.into_response();
    }

    next.run(request).await
}

/// Extract the unit name from the request path to check token permissions against.
fn resolve_access_target(request: &Request) -> Result<String, AuthServiceError> {
    let segments: Vec<_> = request
        .uri()
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    match segments.as_slice() {
        [name] => Ok((*name).to_string()),
        [name, "state"] => Ok((*name).to_string()),
        [name, "log"] => Ok((*name).to_string()),
        _ => Err(AuthServiceError::InvalidRoute),
    }
}

fn bearer_credentials(request: &Request) -> Result<(&str, &str), AuthServiceError> {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .ok_or(AuthServiceError::InvalidToken)?;

    let value = header
        .to_str()
        .map_err(|_| AuthServiceError::InvalidToken)?;

    let raw = value
        .strip_prefix("Bearer ")
        .ok_or(AuthServiceError::InvalidToken)?;

    let (name, token) = raw.split_once(':').ok_or(AuthServiceError::InvalidToken)?;
    if name.is_empty() || token.is_empty() {
        return Err(AuthServiceError::InvalidToken);
    }

    Ok((name, token))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::Request,
    };

    use super::*;

    #[test]
    fn resolve_name_unit() {
        let request = Request::builder()
            .uri("/myapp")
            .body(Body::empty())
            .unwrap();

        assert_eq!(resolve_access_target(&request).unwrap(), "myapp");
    }

    #[test]
    fn resolve_name_unit_state() {
        let request = Request::builder()
            .uri("/myapp/state")
            .body(Body::empty())
            .unwrap();
        assert_eq!(resolve_access_target(&request).unwrap(), "myapp");
    }

    #[test]
    fn resolve_name_empty_path() {
        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        assert!(resolve_access_target(&request).is_err());
    }

    #[test]
    fn resolve_name_unknown_suffix() {
        let request = Request::builder()
            .uri("/myapp/unknown")
            .body(Body::empty())
            .unwrap();
        assert!(resolve_access_target(&request).is_err());
    }

    #[test]
    fn resolve_name_unit_log() {
        let request = Request::builder()
            .uri("/myapp/log")
            .body(Body::empty())
            .unwrap();
        assert_eq!(resolve_access_target(&request).unwrap(), "myapp");
    }

    #[test]
    fn bearer_credentials_ok() {
        let request = Request::builder()
            .header(header::AUTHORIZATION, "Bearer admin:secret")
            .body(Body::empty())
            .unwrap();
        let (name, token) = bearer_credentials(&request).unwrap();
        assert_eq!(name, "admin");
        assert_eq!(token, "secret");
    }

    #[test]
    fn bearer_credentials_no_separator() {
        let request = Request::builder()
            .header(header::AUTHORIZATION, "Bearer adminsecret")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            bearer_credentials(&request),
            Err(AuthServiceError::InvalidToken)
        ));
    }

    #[test]
    fn bearer_credentials_missing_header() {
        let request = Request::builder().body(Body::empty()).unwrap();
        assert!(matches!(
            bearer_credentials(&request),
            Err(AuthServiceError::InvalidToken)
        ));
    }

    #[test]
    fn bearer_credentials_empty_parts() {
        let request = Request::builder()
            .header(header::AUTHORIZATION, "Bearer :secret")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            bearer_credentials(&request),
            Err(AuthServiceError::InvalidToken)
        ));

        let request = Request::builder()
            .header(header::AUTHORIZATION, "Bearer admin:")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            bearer_credentials(&request),
            Err(AuthServiceError::InvalidToken)
        ));
    }
}
