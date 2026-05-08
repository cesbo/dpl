use sha2::{
    Digest,
    Sha256,
};

pub fn bearer_hash(name: &str, token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b":");
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}
