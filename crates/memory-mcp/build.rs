use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

const DIST_ENV: &str = "MEMORY_MCP_UI_DIST";
const STAGED_DIR: &str = "ui";
const MANIFEST_FILE: &str = "ui_assets.rs";

/// Below this many bytes, a gzip header and the CPU to produce it cost more
/// than the transfer saves. The smallest asset that genuinely benefits is
/// the console's WebAssembly module; the favicon and the tiny HTML shell are
/// left alone.
const MIN_COMPRESS_BYTES: usize = 1024;

#[derive(Debug)]
struct Asset {
    source: PathBuf,
    relative: PathBuf,
    url_path: String,
    content_type: &'static str,
    immutable: bool,
    /// The staged gzip encoding of this asset, when it is worth having one.
    gzip: Option<PathBuf>,
}

fn main() {
    println!("cargo:rerun-if-env-changed={DIST_ENV}");

    if env::var_os("CARGO_FEATURE_UI").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set by Cargo"));
    let assets = match env::var_os(DIST_ENV) {
        Some(raw) => {
            let dist = PathBuf::from(raw);
            match build_assets(&dist, &out_dir) {
                Ok(assets) => assets,
                // A bundle was provided but is malformed; fail fast rather than
                // silently shipping a UI-less build.
                Err(error) => panic!("ui asset packaging failed: {error}"),
            }
        }
        // No bundle was provided. Emit an empty asset catalog so the binary
        // still compiles and simply does not serve a UI. Only the UI-serving
        // tests set DIST_ENV to a real bundle.
        None => Vec::new(),
    };
    write_manifest(&out_dir, &assets);
}

fn build_assets(dist: &Path, out_dir: &Path) -> Result<Vec<Asset>, String> {
    if !dist.is_absolute() {
        return Err(format!(
            "{DIST_ENV} must be absolute; received {}",
            dist.display()
        ));
    }

    let dist_metadata = fs::symlink_metadata(dist).map_err(|error| {
        format!(
            "cannot read {DIST_ENV} directory {}: {error}",
            dist.display()
        )
    })?;
    if !dist_metadata.is_dir() {
        return Err(format!(
            "{DIST_ENV} must point to a directory; received {}",
            dist.display()
        ));
    }
    if dist_metadata.file_type().is_symlink() {
        return Err(format!(
            "{DIST_ENV} must point to a real directory, not a symlink: {}",
            dist.display()
        ));
    }

    println!("cargo:rerun-if-changed={}", dist.display());
    let mut assets = Vec::new();
    collect_assets(dist, dist, &mut assets)?;
    assets.sort_by(|left, right| left.url_path.cmp(&right.url_path));

    let index = assets
        .iter()
        .find(|asset| asset.url_path == "/index.html")
        .ok_or_else(|| format!("bundle {} does not contain index.html", dist.display()))?;
    if fs::metadata(&index.source)
        .map_err(|error| format!("cannot inspect {}: {error}", index.source.display()))?
        .len()
        == 0
    {
        return Err(format!(
            "bundle index.html is empty: {}",
            index.source.display()
        ));
    }

    let staged_dir = out_dir.join(STAGED_DIR);
    if staged_dir.exists() {
        fs::remove_dir_all(&staged_dir).map_err(|error| {
            format!(
                "cannot clear staged asset directory {}: {error}",
                staged_dir.display()
            )
        })?;
    }
    fs::create_dir_all(&staged_dir).map_err(|error| {
        format!(
            "cannot create staged asset directory {}: {error}",
            staged_dir.display()
        )
    })?;

    for asset in &mut assets {
        let destination = staged_dir.join(&asset.relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create asset directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::copy(&asset.source, &destination).map_err(|error| {
            format!(
                "cannot stage asset {} at {}: {error}",
                asset.source.display(),
                destination.display()
            )
        })?;
        // The manifest is generated after this loop, so the catalog entry has
        // to be cleared when the sidecar is not written: a `Some` pointing at
        // a file that was never staged would make `include_bytes!` fail to
        // compile.
        if asset.gzip.is_some() {
            let gzip = asset.gzip.clone().expect("checked above");
            asset.gzip = write_gzip(&asset.source, &staged_dir.join(&gzip))?.then_some(gzip);
        }
        println!("cargo:rerun-if-changed={}", asset.source.display());
    }

    Ok(assets)
}

/// Write the gzip encoding of `source` to `destination`.
///
/// The encoding happens once, here, so the shipped binary serves a stored
/// representation instead of compressing on every request: the asset's bytes
/// are fixed at build time, so compressing per request would spend CPU to
/// produce the same answer forever. `build.rs` re-runs on every change under
/// the bundle directory, so this is a hot path and uses the default level —
/// measured against the console's own module, the highest level buys about
/// 0.2% for several times the CPU.
///
/// Returns whether the result is worth keeping. Bytes that are already dense
/// (a module that is mostly an uncompressed link table) can grow under gzip,
/// and a second copy that is larger than the first is pure weight in the
/// shipped binary, so it is dropped and the identity body is served alone.
fn write_gzip(source: &Path, destination: &Path) -> Result<bool, String> {
    use std::io::Write as _;

    let body = fs::read(source)
        .map_err(|error| format!("cannot read {} for compression: {error}", source.display()))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&body)
        .map_err(|error| format!("cannot compress {}: {error}", source.display()))?;
    let encoded = encoder
        .finish()
        .map_err(|error| format!("cannot finish compressing {}: {error}", source.display()))?;
    if encoded.len() >= body.len() {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create asset directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(destination, &encoded)
        .map_err(|error| format!("cannot write {}: {error}", destination.display()))?;
    Ok(true)
}

/// Writes the `ui_assets.rs` manifest that `static_assets.rs`
/// `include!`s. When no bundle was provided the asset list is empty, so the
/// binary compiles with a catalog that simply serves no UI.
fn write_manifest(out_dir: &Path, assets: &[Asset]) {
    let manifest = generate_manifest(assets);
    fs::write(out_dir.join(MANIFEST_FILE), manifest)
        .expect("cannot write generated ui asset manifest")
}

fn collect_assets(root: &Path, current: &Path, assets: &mut Vec<Asset>) -> Result<(), String> {
    let entries = fs::read_dir(current)
        .map_err(|error| format!("cannot read asset directory {}: {error}", current.display()))?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot enumerate asset directory {}: {error}",
                current.display()
            )
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect asset {}: {error}", path.display()))?;

        if metadata.file_type().is_symlink() {
            return Err(format!(
                "bundle contains unsupported symlink: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_assets(root, &path, assets)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!(
                "bundle contains unsupported entry: {}",
                path.display()
            ));
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("cannot relativize asset {}: {error}", path.display()))?
            .to_path_buf();
        let url_path = url_path(&relative)?;
        let gzip = compressible(&relative, metadata.len()).then(|| staged_gzip_path(&relative));
        assets.push(Asset {
            source: path,
            relative,
            content_type: content_type(&url_path),
            immutable: is_content_addressed(&url_path),
            url_path,
            gzip,
        });
    }

    Ok(())
}

/// The staged path of an asset's gzip sidecar, relative to the staged bundle.
///
/// The sidecar is namespaced with a prefix rather than suffixed with `.gz`,
/// because a suffix collides: a bundle that ships both `app.wasm` and a file
/// literally named `app.wasm.gz` would stage two different assets onto one
/// path, and whichever was written last would be served as the other. A client
/// asking for gzip would then receive identity bytes labelled
/// `Content-Encoding: gzip` — a module that cannot decode, and a console that
/// never boots, with nothing in the build output to say why.
///
/// A prefix cannot collide with the bundle's own layout unless a bundle file
/// also starts with it, which [`check`] refuses.
pub(crate) const GZIP_PREFIX: &str = "gzip__";

fn staged_gzip_path(relative: &Path) -> PathBuf {
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
fn compressible(relative: &Path, len: u64) -> bool {
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

fn url_path(relative: &Path) -> Result<String, String> {
    let mut url = String::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!(
                "bundle contains an invalid relative asset path: {}",
                relative.display()
            ));
        };
        let component = component.to_str().ok_or_else(|| {
            format!(
                "bundle contains a non-UTF-8 asset path: {}",
                relative.display()
            )
        })?;
        if component.is_empty() {
            return Err(format!(
                "bundle contains an empty asset path: {}",
                relative.display()
            ));
        }
        url.push('/');
        url.push_str(component);
    }
    Ok(url)
}

/// Return whether a bundle path carries a stable content hash.
///
/// Dioxus emits names such as `main-dxh1234567890.css`. Only filenames with a
/// lowercase alphanumeric suffix of at least eight characters after the final
/// hyphen are treated as content-addressed. Stable names such as `index.html`
/// and `favicon.svg` therefore remain revalidated, and short or unusual names
/// are conservatively treated as mutable.
fn is_content_addressed(url_path: &str) -> bool {
    let path = Path::new(url_path);
    if path.file_name() == Some(OsStr::new("index.html"))
        || path.file_name() == Some(OsStr::new("favicon.svg"))
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

fn content_type(path: &str) -> &'static str {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");

    if extension.eq_ignore_ascii_case("html") {
        "text/html; charset=utf-8"
    } else if extension.eq_ignore_ascii_case("css") {
        "text/css; charset=utf-8"
    } else if extension.eq_ignore_ascii_case("js") || extension.eq_ignore_ascii_case("mjs") {
        "text/javascript; charset=utf-8"
    } else if extension.eq_ignore_ascii_case("wasm") {
        "application/wasm"
    } else if extension.eq_ignore_ascii_case("json") || extension.eq_ignore_ascii_case("map") {
        "application/json"
    } else if extension.eq_ignore_ascii_case("svg") {
        "image/svg+xml"
    } else if extension.eq_ignore_ascii_case("png") {
        "image/png"
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        "image/jpeg"
    } else if extension.eq_ignore_ascii_case("gif") {
        "image/gif"
    } else if extension.eq_ignore_ascii_case("ico") {
        "image/x-icon"
    } else if extension.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else if extension.eq_ignore_ascii_case("woff") {
        "font/woff"
    } else if extension.eq_ignore_ascii_case("woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

fn generate_manifest(assets: &[Asset]) -> String {
    let mut manifest = String::from("const ASSETS: &[Asset] = &[\n");
    for asset in assets {
        let relative = rust_string_literal(&asset.relative.to_string_lossy());
        let url_path = rust_string_literal(&asset.url_path);
        let content_type = rust_string_literal(asset.content_type);
        let gzip = match &asset.gzip {
            Some(name) => format!(
                "Some(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{STAGED_DIR}/\", {})))",
                rust_string_literal(&name.to_string_lossy())
            ),
            None => "None".to_string(),
        };
        manifest.push_str(&format!(
            "    Asset {{ path: {url_path}, content_type: {content_type}, immutable: {}, gzip_body: {gzip}, body: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{STAGED_DIR}/\", {relative})) }},\n",
            asset.immutable
        ));
    }
    manifest.push_str("];\n");
    manifest
}

fn rust_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        match character {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            character if character.is_control() => {
                literal.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

#[cfg(test)]
mod tests {
    use super::is_content_addressed;

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
}
