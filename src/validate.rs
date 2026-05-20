use crate::config::ResourceName;

pub fn secret_name(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }

    path.split('/').all(ResourceName::is_valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_name_accepts_valid() {
        assert!(secret_name("foo"));
        assert!(secret_name("foo/bar"));
        assert!(secret_name("a-b-c"));
        assert!(secret_name("db/prod-password"));
        assert!(secret_name("a/b/c"));
    }

    #[test]
    fn secret_name_rejects_invalid() {
        assert!(!secret_name(""));
        assert!(!secret_name("/foo"));
        assert!(!secret_name("foo/"));
        assert!(!secret_name("foo//bar"));
        assert!(!secret_name("foo/../bar"));
    }
}
