//! How a bundle asset is named, classified, and compressed.
//!
//! This lives outside `build.rs` for one reason: Cargo never compiles a build
//! script's `#[cfg(test)]` module, so tests written there do not run. The
//! decisions below are the ones that decide what a client is actually served —
//! whether an asset is cached immutably, and whether it carries a compressed
//! second copy — so they are worth testing, and they are testable only from a
//! module Cargo compiles normally.
//!
//! The functions are pure: they take a path or a length and answer a question.
//! Staging and manifest generation stay in `build.rs`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Below this many bytes, a gzip header and the CPU to produce it cost more
/// than the transfer saves. The smallest asset that genuinely benefits is the
/// console's WebAssembly module; the favicon and the tiny HTML shell are left
/// alone.
pub const MIN_COMPRESS_BYTES: usize = 1024;

/// The prefix a gzip sidecar is namespaced under.
///
/// A prefix rather than a `.gz` suffix, because a suffix collides: a bundle
/// that ships both `app.wasm` and a file literally named `app.wasm.gz` would
/// stage two different assets onto one path, and whichever was written last
/// would be served as the other. A client asking for gzip would then receive
/// identity bytes labelled `Content-Encoding: gzip` — a module that cannot
/// decode, and a console that never boots, with nothing in the build output to
/// say why.
///
/// A prefix cannot collide with the bundle's own layout unless a bundle file
/// also starts with it, which `xtask check-ui-bundle` refuses.
pub const GZIP_PREFIX: &str = "gzip__";

/// The staged path of an asset's gzip sidecar, relative to the staged bundle.
///
/// It is a sibling of the asset it encodes, keeping the original's extension
/// and directory.
pub fn staged_gzip_path(relative: &Path) -> PathBuf {
    let mut name = OsString::from(GZIP_PREFIX);
    name.push(relative.file_name().unwrap_or_default());
    relative.with_file_name(name)
}

/// Whether an asset is worth carrying a second, compressed copy of itself.
///
/// This is an allowlist, not a denylist. A denylist has to enumerate every
/// compressed format in existence, and every one it misses silently doubles
/// the shipped binary for nothing; an allowlist fails the other way, leaving an
/// unusual but legitimate type uncompressed, which costs only a few bytes on
/// one request.
///
/// An asset that is itself an encoding of another — anything ending in `.gz`,
/// `.br`, or a media container — is excluded, so a bundle that already ships
/// pre-compressed files is never compressed twice.
pub fn compressible(relative: &Path, len: u64) -> bool {
    if len < MIN_COMPRESS_BYTES as u64 {
        return false;
    }
    let Some(extension) = relative.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        // Text and code a browser fetches and decodes itself.
        "js" | "mjs" | "css" | "html" | "htm" | "json" | "map" | "xml" | "txt" | "svg" | "wasm"
    )
}

/// Return whether a bundle path carries a stable content hash.
///
/// Dioxus emits names such as `main-dxh1234567890.css`. Only filenames with a
/// lowercase alphanumeric suffix of at least eight characters after the final
/// hyphen are treated as content-addressed. Stable names such as `index.html`
/// and `favicon.svg` therefore remain revalidated, and short or unusual names
/// are conservatively treated as mutable.
///
/// Getting this wrong in the permissive direction pins every client to the
/// assets of one build: an immutable header on a name that later holds
/// different bytes is never revalidated.
pub fn is_content_addressed(url_path: &str) -> bool {
    let path = Path::new(url_path);
    if path.file_name() == Some(OsString::from("index.html").as_os_str())
        || path.file_name() == Some(OsString::from("favicon.svg").as_os_str())
    {
        return false;
    }

    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((_, suffix)) = stem.rsplit_once('-') else {
        return false;
    };

    suffix.len() >= 8
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether a gzip encoding of `body` is worth storing alongside it.
///
/// This is the "did compression actually help" question, kept separate from
/// the writing so it can be answered without touching the filesystem — and
/// tested directly, which matters because the answer decides whether the
/// shipped binary carries a second copy of an asset or not.
///
/// Bytes that are already dense (a module that is mostly an uncompressed link
/// table) can grow under gzip, and a second copy larger than the first is pure
/// weight in the binary.
pub fn is_worth_compressing(body: &[u8]) -> bool {
    use std::io::Write as _;
    if body.len() < MIN_COMPRESS_BYTES {
        return false;
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to memory cannot fail, so the result is only discarded here; the
    // real call in `build.rs` writes the sidecar to a file.
    if encoder.write_all(body).is_err() {
        return false;
    }
    encoder
        .finish()
        .is_ok_and(|encoded| encoded.len() < body.len())
}

/// The gzip encoding of `body`, at the level the build uses.
///
/// The default level rather than the highest: `build.rs` re-runs on every
/// change under the bundle directory, and measured against the console's own
/// module the highest level buys about 0.2% for several times the CPU.
pub fn gzip_encode(body: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(body)
        .map_err(|error| format!("cannot compress an asset: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("cannot finish compressing an asset: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        MIN_COMPRESS_BYTES, compressible, gzip_encode, is_content_addressed, is_worth_compressing,
        staged_gzip_path,
    };
    use std::path::Path;

    /// Bytes a compressor cannot shrink. A linear-congruential generator is not
    /// enough here: its low bits repeat often enough to compress, which would
    /// make this fixture pass the "worth compressing" check for the wrong
    /// reason.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut state = 0x1234_5678u32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    #[test]
    fn recognizes_dioxus_hashed_assets() {
        assert!(is_content_addressed("/assets/ui-dxh395eca31249da547.js"));
        assert!(is_content_addressed("/assets/main-dxh8aea88cdab71b47.css"));
    }

    #[test]
    fn keeps_stable_and_ambiguous_names_revalidating() {
        for path in [
            "/index.html",
            "/assets/favicon.svg",
            "/assets/main.css",
            "/assets/main-short.css",
            "/assets/main-dxH395eca31249da547.js",
        ] {
            assert!(!is_content_addressed(path), "path: {path}");
        }
    }

    /// The sidecar must never land on a path the bundle itself occupies. A
    /// `.gz` suffix collided: a bundle carrying both `app.wasm` and a file
    /// named `app.wasm.gz` staged two assets onto one path, and the binary
    /// served one asset's bytes under the other's name — a module that cannot
    /// decode, labelled as though it could.
    #[test]
    fn a_gzip_sidecar_never_lands_on_a_bundle_path() {
        assert_ne!(
            staged_gzip_path(Path::new("assets/app.wasm")),
            Path::new("assets/app.wasm.gz")
        );
        // A bundle-root asset has no parent directory to keep the two apart,
        // so the prefix has to do the work on its own.
        assert_ne!(
            staged_gzip_path(Path::new("index.html")),
            Path::new("index.html.gz")
        );
        // The sidecar stays a sibling of the asset it encodes, keeping the
        // original's extension and directory.
        assert_eq!(
            staged_gzip_path(Path::new("assets/app.wasm")),
            Path::new("assets/gzip__app.wasm")
        );
    }

    /// A type that is already an encoding of something else gains nothing from
    /// a second copy, and a format the allowlist does not know about must fail
    /// toward *not* being compressed rather than silently doubling the binary.
    #[test]
    fn only_decodable_types_above_the_floor_are_compressed() {
        let large = MIN_COMPRESS_BYTES as u64;
        for path in [
            "app.js",
            "app.mjs",
            "main.css",
            "index.html",
            "app_bg.wasm",
            "data.json",
            "icon.svg",
        ] {
            assert!(compressible(Path::new(path), large), "path: {path}");
        }
        for path in [
            "app.gz",
            "app.br",
            "photo.png",
            "photo.avif",
            "clip.mp4",
            "font.woff2",
        ] {
            assert!(!compressible(Path::new(path), large), "path: {path}");
        }
        // Under the floor nothing earns a second copy: the gzip header alone
        // would be a meaningful fraction of the asset.
        assert!(!compressible(Path::new("app.js"), large - 1));
        // An asset with no extension has no type to reason about.
        assert!(!compressible(Path::new("LICENSE"), large));
    }

    /// A sidecar is only shipped when it is smaller than the asset it encodes.
    /// Getting this backwards is not a size win, it is a bug: the catalog would
    /// reference a sidecar that was never written.
    #[test]
    fn a_body_that_does_not_shrink_earns_no_sidecar() {
        assert!(
            !is_worth_compressing(&incompressible(8192)),
            "incompressible bytes must not earn a sidecar"
        );
        // At and just under the floor, a sidecar is not worth having: the gzip
        // header dominates, and the answer must not depend on the exact byte.
        let floor = MIN_COMPRESS_BYTES;
        assert!(!is_worth_compressing(&incompressible(floor)));
        assert!(!is_worth_compressing(&incompressible(floor - 1)));
    }

    #[test]
    fn a_compressible_body_earns_a_sidecar_that_decodes_back_to_it() {
        use std::io::Read as _;

        // Long enough that the gzip header is a small fraction of it: a short
        // string compresses to *more* than it started as.
        let body = "the quick brown fox jumps over the lazy dog. ".repeat(64);
        assert!(is_worth_compressing(body.as_bytes()));

        let encoded = gzip_encode(body.as_bytes()).expect("encodes");
        assert!(
            encoded.len() < body.len(),
            "the sidecar must be smaller: {} vs {}",
            encoded.len(),
            body.len()
        );
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&encoded[..])
            .read_to_end(&mut decoded)
            .expect("valid gzip");
        assert_eq!(
            decoded,
            body.as_bytes(),
            "the sidecar must encode the source exactly"
        );
    }
}
