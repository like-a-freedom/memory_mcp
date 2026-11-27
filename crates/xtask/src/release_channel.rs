//! Stable-channel promotion policy, independent of registry transport.
use crate::pack::PackError;

fn refused(message: &str) -> PackError {
    PackError::Bundle(format!("stable channel promotion refused: {message}"))
}

struct Version {
    core: [u64; 3],
    prerelease: bool,
}

fn identifiers(value: &str, prerelease: bool) -> bool {
    value.split('.').all(|part| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !(prerelease
                && part.len() > 1
                && part.starts_with('0')
                && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn version(tag: &str) -> Result<Version, PackError> {
    let value = tag
        .strip_prefix('v')
        .ok_or_else(|| refused("version must start with v"))?;
    let base = if let Some((base, build)) = value.split_once('+') {
        if !identifiers(build, false) {
            return Err(refused("invalid build metadata"));
        }
        base
    } else {
        value
    };
    let (core, prerelease) = if let Some((core, pre)) = base.split_once('-') {
        if !identifiers(pre, true) {
            return Err(refused("invalid prerelease"));
        }
        (core, true)
    } else {
        (base, false)
    };
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(refused("version requires three numeric components"));
    }
    let mut numbers = [0; 3];
    for (number, part) in numbers.iter_mut().zip(parts) {
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(refused("invalid numeric version component"));
        }
        *number = part
            .parse()
            .map_err(|_| refused("version component overflow"))?;
    }
    Ok(Version {
        core: numbers,
        prerelease,
    })
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Decide whether to advance stable latest while refusing digest conflicts.
pub fn choose(
    candidate_tag: &str,
    current: Option<(&str, &str)>,
    candidate_digest: &str,
) -> Result<bool, PackError> {
    let candidate = version(candidate_tag)?;
    if !digest(candidate_digest) {
        return Err(refused("invalid candidate digest"));
    }
    if candidate.prerelease {
        return Ok(false);
    }
    let Some((current_tag, current_digest)) = current else {
        return Ok(true);
    };
    let current = version(current_tag)?;
    if current.prerelease || !digest(current_digest) {
        return Err(refused("invalid current stable channel"));
    }
    match candidate.core.cmp(&current.core) {
        std::cmp::Ordering::Greater => Ok(true),
        std::cmp::Ordering::Less => Ok(false),
        std::cmp::Ordering::Equal if candidate_digest == current_digest => Ok(false),
        std::cmp::Ordering::Equal => Err(refused("same version has a different image digest")),
    }
}

#[cfg(test)]
mod tests {
    use super::choose;
    const D: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const E: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    #[test]
    fn promotes_first_stable_release() {
        assert!(choose("v2.0.0", None, D).unwrap());
    }
    #[test]
    fn promotes_newer_release() {
        assert!(choose("v2.1.0", Some(("v2.0.9", D)), E).unwrap());
    }
    #[test]
    fn compares_numbers_not_strings() {
        assert!(choose("v2.10.0", Some(("v2.9.0", D)), E).unwrap());
    }
    #[test]
    fn skips_older_release() {
        assert!(!choose("v2.0.1", Some(("v2.1.0", D)), E).unwrap());
    }
    #[test]
    fn same_release_same_digest_is_idempotent() {
        assert!(!choose("v2.0.0", Some(("v2.0.0", D)), D).unwrap());
    }
    #[test]
    fn same_release_different_digest_is_refused() {
        assert!(choose("v2.0.0", Some(("v2.0.0", D)), E).is_err());
    }
    #[test]
    fn never_promotes_prerelease() {
        assert!(!choose("v3.0.0-rc.1", Some(("v2.0.0", D)), E).unwrap());
    }
    #[test]
    fn build_metadata_does_not_change_precedence() {
        assert!(!choose("v2.0.0+build.1", Some(("v2.0.0+build.0", D)), D).unwrap());
    }
    #[test]
    fn rejects_invalid_candidate_version() {
        assert!(choose("v2.01.0", None, D).is_err());
    }
    #[test]
    fn rejects_missing_v_prefix() {
        assert!(choose("2.0.0", None, D).is_err());
    }
    #[test]
    fn rejects_invalid_numeric_prerelease() {
        assert!(choose("v2.0.0-rc.01", None, D).is_err());
    }
    #[test]
    fn rejects_empty_build_identifier() {
        assert!(choose("v2.0.0+build..1", None, D).is_err());
    }
    #[test]
    fn rejects_unknown_current_version() {
        assert!(choose("v2.0.0", Some(("latest", D)), D).is_err());
    }
    #[test]
    fn rejects_prerelease_current_channel() {
        assert!(choose("v2.0.0", Some(("v1.0.0-rc.1", D)), D).is_err());
    }
    #[test]
    fn rejects_invalid_candidate_digest() {
        assert!(choose("v2.0.0", None, "broken").is_err());
    }
    #[test]
    fn rejects_invalid_current_digest() {
        assert!(choose("v2.0.0", Some(("v1.0.0", "broken")), D).is_err());
    }
    #[test]
    fn rejects_version_overflow() {
        assert!(choose("v999999999999999999999999.0.0", None, D).is_err());
    }
}
