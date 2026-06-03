use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    reference::ReferenceError,
};

const DEFAULT_IMAGE: &str = "docker.io/library/nginx:stable";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HttpServerConfig {
    #[serde(default = "default_image")]
    pub image: String,

    #[serde(default)]
    pub https: bool,
}

fn default_image() -> String {
    DEFAULT_IMAGE.to_string()
}

impl HttpServerConfig {
    pub fn validate_references(&self, _ctx: &MainContext) -> Result<(), ReferenceError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config_uses_defaults() {
        let config: HttpServerConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(config.image, DEFAULT_IMAGE);
        assert!(!config.https);
    }

    #[test]
    fn parse_full_config() {
        let config: HttpServerConfig = serde_yaml::from_str(
            r#"
image: docker.io/library/nginx:1.27
https: true
"#,
        )
        .unwrap();
        assert_eq!(config.image, "docker.io/library/nginx:1.27");
        assert!(config.https);
    }

    #[test]
    fn reject_unknown_field() {
        let err = serde_yaml::from_str::<HttpServerConfig>("port: 80\n").unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }
}
