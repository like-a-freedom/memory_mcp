//! The toolchain the release artefacts are actually built with.
//!
//! Three files each name a Rust version, and they mean different things:
//!
//!   * `rust-toolchain.toml` — the channel a developer and every `cargo`
//!     invocation in the repository resolves to. This is the one that decides
//!     what is built.
//!   * `Dockerfile` — the `FROM` tag. `COPY . .` copies the toolchain file into
//!     `/src`, and rustup honours it over the image default, so this tag does not
//!     decide the compiler; it decides what the image ships before the pin is
//!     read, and it is the version an operator sees.
//!   * `.github/actions/setup/action.yml` — the channel CI builds on.
//!
//! `rust-version` in `Cargo.toml` names the same channel today, and is not one
//! of these three: it is what a crate may be built by, not what CI and the
//! image are built by.
//!
//! These drifted apart before. The image tag stayed at `1.97.1` through a
//! `rust-toolchain.toml` move to 1.99, which is what let the console bundle
//! build against a different LLVM than the tag claimed — see the
//! `[profile.wasm-release]` note in the workspace `Cargo.toml`. Nothing failed
//! at the bump: a tag behind the pin is silently overridden, so the only symptom
//! appeared much later, in the bundler, naming a file that did not exist. So the
//! three are required to be equal here, where a bump has to touch them.

use crate::pack::PackError;

const TOOLCHAIN_FILE: &str = "rust-toolchain.toml";
const DOCKERFILE: &str = "Dockerfile";
const SETUP_ACTION: &str = ".github/actions/setup/action.yml";

/// Reads the toolchain one file names, or `None` when it names none.
type VersionReader = fn(&str) -> Option<String>;

/// The three files that name the toolchain the artefacts are built with.
const SOURCES: [(&str, VersionReader); 3] = [
    (TOOLCHAIN_FILE, toolchain_channel),
    (DOCKERFILE, dockerfile_base),
    (SETUP_ACTION, setup_toolchain),
];

/// Require every file that names the build toolchain to name the same one.
///
/// A file that is not present is skipped rather than failed: `.github` is
/// excluded from the Docker build context, so the setup action genuinely does
/// not exist inside the image, and the check runs in that context too. The
/// workflow job that can see the action is the one that enforces it.
pub fn check() -> Result<(), PackError> {
    let channel = toolchain_channel(&read_repository_file(TOOLCHAIN_FILE)?)
        .ok_or_else(|| PackError::Bundle(format!("{TOOLCHAIN_FILE} pins no channel")))?;

    for (path, parse) in SOURCES {
        // Absent is skipped, not failed: `.github` is outside the Docker build
        // context, so the image genuinely cannot see the workflow config. The
        // job that can is the one that enforces it.
        let Ok(contents) = read_repository_file(path) else {
            continue;
        };
        require(path, &contents, parse, &channel)?;
    }
    Ok(())
}

/// The comparison itself, so it can be exercised over sources the caller
/// supplies rather than only over the real repository.
fn require(
    path: &str,
    contents: &str,
    parse: VersionReader,
    channel: &str,
) -> Result<(), PackError> {
    let found = parse(contents);
    if found.as_deref() == Some(channel) {
        return Ok(());
    }
    Err(PackError::Bundle(format!(
        "{path} names {}, but {TOOLCHAIN_FILE} pins {channel}. {TOOLCHAIN_FILE} \
         wins at build time — rustup resolves it over the image default and over \
         any other selection — so the others are documentation that is already \
         wrong. Update them together.",
        found.as_deref().unwrap_or("no single toolchain")
    )))
}

/// The channel `rust-toolchain.toml` pins.
fn toolchain_channel(file: &str) -> Option<String> {
    file.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("channel")?.trim_start().strip_prefix('='))
        .map(|value| value.trim().trim_matches('"').to_owned())
}

/// The `rust:<version>` the `Dockerfile` builds from. Every stage has to agree:
/// a `ui-builder` and a `builder` on different channels would compile the console
/// and the binary that embeds it under different toolchains.
///
/// `None` covers disagreement as well as absence, so a stage left behind cannot
/// read as a version the caller then compares and accepts.
fn dockerfile_base(dockerfile: &str) -> Option<String> {
    let mut versions: Vec<&str> = dockerfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("FROM rust:"))
        .filter_map(|tag| tag.split('-').next())
        .filter(|version| !version.is_empty())
        .collect();

    // `dedup` only collapses neighbours, so a stage repeated after a differing
    // one still leaves more than one distinct version behind.
    versions.sort_unstable();
    versions.dedup();

    match versions.as_slice() {
        [only] => Some((*only).to_owned()),
        // Absent, or stages that disagree: neither is a version to compare.
        [] | [_, _, ..] => None,
    }
}

/// The channel CI builds on.
fn setup_toolchain(action: &str) -> Option<String> {
    action.lines().find_map(|line| {
        let value = line.trim().strip_prefix("toolchain:")?.trim();
        (!value.is_empty()).then(|| value.trim_matches('"').to_owned())
    })
}

fn read_repository_file(relative: &str) -> Result<String, PackError> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| {
            PackError::Bundle("cannot locate the repository root from the manifest".into())
        })?;
    let path = root.join(relative);
    std::fs::read_to_string(&path)
        .map_err(|error| PackError::Bundle(format!("cannot read {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real repository: the image and CI must build on the channel a
    /// checkout resolves to.
    #[test]
    fn every_file_names_the_pinned_toolchain() {
        check().expect("the image, CI and the toolchain file must agree");
    }

    /// The parsers, so the check above cannot pass by finding nothing in the
    /// files that are supposed to name a version.
    #[test]
    fn the_pin_is_read_from_every_source() {
        assert_eq!(
            toolchain_channel("channel = \"1.99\"\n").as_deref(),
            Some("1.99")
        );
        assert_eq!(toolchain_channel("[toolchain]\ncomponents = []\n"), None);

        // The suffix and the registry are both dropped: `1.99-slim-trixie` and
        // `1.99-bookworm` are the same toolchain.
        assert_eq!(
            dockerfile_base("FROM rust:1.99-slim-trixie AS builder\n").as_deref(),
            Some("1.99")
        );
        assert_eq!(
            dockerfile_base("FROM rust:1.99-bookworm AS builder\n").as_deref(),
            Some("1.99")
        );
        assert_eq!(
            dockerfile_base("FROM gcr.io/distroless/cc AS runtime\n"),
            None
        );

        assert_eq!(
            setup_toolchain("        toolchain: 1.99\n").as_deref(),
            Some("1.99")
        );
        assert_eq!(setup_toolchain("      components: rustfmt\n"), None);
    }

    /// One stage left behind is the drift this check exists for, so it has to
    /// read as disagreement rather than as a version it can average away.
    #[test]
    fn a_stage_left_on_another_toolchain_is_not_a_match() {
        let mixed = "FROM rust:1.99-slim-trixie AS ui-builder\n\
                     FROM rust:1.97.1-slim-trixie AS builder\n";
        assert_eq!(dockerfile_base(mixed), None);

        // Agreement across both stages is still agreement.
        let agreed = "FROM rust:1.99-slim-trixie AS ui-builder\n\
                      FROM rust:1.99-slim-trixie AS builder\n";
        assert_eq!(dockerfile_base(agreed).as_deref(), Some("1.99"));
    }

    /// The comparison the check performs, over sources the caller supplies. The
    /// real repository cannot show the absent case — this checkout has the
    /// workflow config — so the rule is exercised directly instead.
    #[test]
    fn the_rule_accepts_a_matching_source_and_refuses_a_lagging_one() {
        let agreeing = "FROM rust:1.99-slim-trixie AS ui-builder\n\
                        FROM rust:1.99-slim-trixie AS builder\n";
        require(DOCKERFILE, agreeing, dockerfile_base, "1.99")
            .expect("a tag that matches the pin must pass");

        let lagging = "FROM rust:1.99-slim-trixie AS ui-builder\n\
                       FROM rust:1.97.1-slim-trixie AS builder\n";
        let error = require(DOCKERFILE, lagging, dockerfile_base, "1.99")
            .expect_err("stages that disagree cannot be compared against the pin");
        assert!(error.to_string().contains(DOCKERFILE), "{error}");

        // What the image sees: no workflow config. `check` skips an unreadable
        // source, so this has to read as absent rather than as a mismatch.
        assert_eq!(setup_toolchain("").as_deref(), None);
    }
}
