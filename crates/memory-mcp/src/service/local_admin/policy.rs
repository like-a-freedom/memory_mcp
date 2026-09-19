use crate::service::local_admin::contracts::{LocalAdminError, LocalResult};

/// Normalize a username: trim outer ASCII whitespace, lowercase ASCII.
/// Valid: ASCII lowercase `[a-z0-9][a-z0-9._-]{2,63}`.
pub fn normalize_username(raw: &str) -> LocalResult<String> {
    let trimmed = raw.trim();
    let lowered: String = trimmed
        .chars()
        .map(|c| if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c })
        .collect();
    if lowered.is_empty() {
        return Err(LocalAdminError::InvalidInput("username must not be empty".into()));
    }
    if lowered.len() < 3 || lowered.len() > 64 {
        return Err(LocalAdminError::InvalidInput("username must be 3-64 characters".into()));
    }
    if !lowered.is_ascii() {
        return Err(LocalAdminError::InvalidInput(
            "username must be ASCII".into(),
        ));
    }
    let first = lowered.as_bytes()[0];
    if !matches!(first, b'a'..=b'z' | b'0'..=b'9') {
        return Err(LocalAdminError::InvalidInput(
            "username must start with a-z or 0-9".into(),
        ));
    }
    for &b in lowered.as_bytes() {
        if !matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-') {
            return Err(LocalAdminError::InvalidInput(
                "username contains invalid characters".into(),
            ));
        }
    }
    Ok(lowered)
}

/// Validate a password: 15-128 Unicode scalar values, max 1024 UTF-8 bytes,
/// no NUL.
pub fn validate_password(password: &str) -> LocalResult<()> {
    if password.contains('\0') {
        return Err(LocalAdminError::InvalidInput(
            "password must not contain NUL".into(),
        ));
    }
    let char_count = password.chars().count();
    if char_count < 15 {
        return Err(LocalAdminError::InvalidInput(
            "password must be at least 15 characters".into(),
        ));
    }
    if char_count > 128 {
        return Err(LocalAdminError::InvalidInput(
            "password must be at most 128 characters".into(),
        ));
    }
    if password.len() > 1024 {
        return Err(LocalAdminError::InvalidInput(
            "password must not exceed 1024 UTF-8 bytes".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{normalize_username, validate_password};

    #[test]
    fn local_admin_username_policy() {
        assert_eq!(normalize_username("  Ops.One  ").expect("valid"), "ops.one");
        for raw in ["ab", "a b", "аdmin", "a\nadmin", "-admin"] {
            assert!(normalize_username(raw).is_err());
        }
        assert!(normalize_username(&"a".repeat(64)).is_ok());
        assert!(normalize_username(&"a".repeat(65)).is_err());
    }

    #[test]
    fn local_admin_password_policy() {
        assert!(validate_password(&"a".repeat(14)).is_err());
        assert!(validate_password(&"a".repeat(15)).is_ok());
        assert!(validate_password(&"界".repeat(128)).is_ok());
        assert!(validate_password(&"a".repeat(129)).is_err());
        assert!(validate_password("a valid password\0").is_err());
        assert!(validate_password("              x").is_ok());
    }
}
