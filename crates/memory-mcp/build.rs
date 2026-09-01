use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

const DIST_ENV: &str = "MEMORY_MCP_CONTROL_PLANE_UI_DIST";
const STAGED_DIR: &str = "control-plane-ui";
const MANIFEST_FILE: &str = "control_plane_assets.rs";

#[derive(Debug)]
struct Asset {
    source: PathBuf,
    relative: PathBuf,
    url_path: String,
    content_type: &'static str,
}

fn main() {
    println!("cargo:rerun-if-env-changed={DIST_ENV}");

    if env::var_os("CARGO_FEATURE_CONTROL_PLANE_UI").is_none() {
        return;
    }

    if let Err(error) = build_assets() {
        panic!("control-plane-ui asset packaging failed: {error}");
    }
}

fn build_assets() -> Result<(), String> {
    let raw_dist = env::var_os(DIST_ENV).ok_or_else(|| {
        format!("{DIST_ENV} must point to an absolute Dioxus 0.7 web bundle directory")
    })?;
    let dist = PathBuf::from(raw_dist);

    if !dist.is_absolute() {
        return Err(format!(
            "{DIST_ENV} must be absolute; received {}",
            dist.display()
        ));
    }

    let dist_metadata = fs::symlink_metadata(&dist).map_err(|error| {
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
    collect_assets(&dist, &dist, &mut assets)?;
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

    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").ok_or_else(|| "OUT_DIR is not set by Cargo".to_owned())?,
    );
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

    for asset in &assets {
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
        println!("cargo:rerun-if-changed={}", asset.source.display());
    }

    let manifest = generate_manifest(&assets);
    fs::write(out_dir.join(MANIFEST_FILE), manifest).map_err(|error| {
        format!(
            "cannot write generated asset manifest {}: {error}",
            out_dir.join(MANIFEST_FILE).display()
        )
    })?;

    Ok(())
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
        assets.push(Asset {
            source: path,
            relative,
            content_type: content_type(&url_path),
            url_path,
        });
    }

    Ok(())
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
        manifest.push_str(&format!(
            "    Asset {{ path: {url_path}, content_type: {content_type}, body: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{STAGED_DIR}/\", {relative})) }},\n"
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
