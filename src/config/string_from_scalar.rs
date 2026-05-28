use std::fmt;

use serde::de::{
    self,
    Visitor,
};

/// `#[serde(deserialize_with = ...)]` helper that accepts any YAML scalar
/// (string, integer, float, bool) and returns it as `String`. Lets fields
/// like `version: 12` or `version: 8.4` work without quoting.
pub fn deserialize_string_from_scalar<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ScalarVisitor;

    impl<'de> Visitor<'de> for ScalarVisitor {
        type Value = String;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string or number")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_owned())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<String, E> {
            Ok(v.to_string())
        }
    }

    deserializer.deserialize_any(ScalarVisitor)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Wrap {
        #[serde(deserialize_with = "deserialize_string_from_scalar")]
        v: String,
    }

    #[test]
    fn accepts_string() {
        let w: Wrap = serde_yaml::from_str("v: \"18\"").unwrap();
        assert_eq!(w.v, "18");
    }

    #[test]
    fn accepts_integer() {
        let w: Wrap = serde_yaml::from_str("v: 12").unwrap();
        assert_eq!(w.v, "12");
    }

    #[test]
    fn accepts_float() {
        let w: Wrap = serde_yaml::from_str("v: 8.4").unwrap();
        assert_eq!(w.v, "8.4");
    }
}
