pub fn resource_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    if name.starts_with('-') || name.ends_with('-') {
        return false;
    }

    if name.contains("--") {
        return false;
    }

    name.as_bytes()
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn env_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }

    name.as_bytes()
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn url_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }

    if !path.starts_with('/') || path.ends_with('/') {
        return false;
    }

    for segment in path[1 ..].split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return false;
        }

        let valid = segment
            .as_bytes()
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
        if !valid {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_name_accepts_valid() {
        assert!(env_name("FOO"));
        assert!(env_name("foo_bar"));
        assert!(env_name("_private"));
        assert!(env_name("X1"));
        assert!(env_name("MIXED_Case_42"));
    }

    #[test]
    fn env_name_rejects_invalid() {
        assert!(!env_name(""));
        assert!(!env_name("1FOO"));
        assert!(!env_name("FOO-BAR"));
        assert!(!env_name("FOO BAR"));
        assert!(!env_name("FOO.BAR"));
        assert!(!env_name("ÜMLAUT"));
    }

    #[test]
    fn resource_name_rejects_invalid() {
        assert!(!resource_name(""));
        assert!(!resource_name("."));
        assert!(!resource_name("foo/bar"));
        assert!(!resource_name("foo_bar"));
        assert!(!resource_name("foo.bar"));
        assert!(!resource_name("foo--bar"));
        assert!(!resource_name(" foo"));
        assert!(!resource_name("Ümlaut"));
        assert!(!resource_name("FOO"));
        assert!(!resource_name("-foo"));
        assert!(!resource_name("foo-"));
    }

    #[test]
    fn url_path_accepts_valid() {
        assert!(url_path("/"));
        assert!(url_path("/api"));
        assert!(url_path("/api/v1"));
        assert!(url_path("/a-b_c.d"));
        assert!(url_path("/a/b/c"));
    }

    #[test]
    fn url_path_rejects_invalid() {
        assert!(!url_path(""));
        assert!(!url_path("api"));
        assert!(!url_path("/api/"));
        assert!(!url_path("//"));
        assert!(!url_path("/api//v1"));
        assert!(!url_path("/."));
        assert!(!url_path("/.."));
        assert!(!url_path("/api/.."));
        assert!(!url_path("/api/./v1"));
        assert!(!url_path("/ api"));
        assert!(!url_path("/api?x=1"));
        assert!(!url_path("/api#f"));
        assert!(!url_path("/Ümlaut"));
    }
}
