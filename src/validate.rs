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
