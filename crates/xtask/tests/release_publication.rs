//! Filesystem/process integration controls for the release publisher.
#![cfg(unix)]
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const IMAGE: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
struct Publisher {
    dir: tempfile::TempDir,
    version: String,
}
impl Publisher {
    fn new(version: &str) -> Self {
        let p = Self {
            dir: tempfile::tempdir().expect("scratch"),
            version: version.into(),
        };
        for name in ["bin", "dist", "image", "remote", "tmp"] {
            fs::create_dir(p.path(name)).expect("directory");
        }
        for target in [
            "x86_64-unknown-linux-gnu.tar.gz",
            "aarch64-apple-darwin.tar.gz",
            "x86_64-pc-windows-msvc.zip",
            "aarch64-pc-windows-msvc.zip",
        ] {
            let name = format!("memory_mcp-{target}");
            fs::write(p.path(&format!("dist/{name}")), b"qualified archive bytes")
                .expect("archive");
            fs::write(
                p.path(&format!("dist/{name}.sha256")),
                format!(
                    "{}\n",
                    hex::encode(Sha256::digest(b"qualified archive bytes"))
                ),
            )
            .expect("sidecar");
        }
        p.xtask(&[
            "release-bundle",
            "seal",
            p.path("dist").to_str().expect("path"),
            "--source-sha",
            SOURCE,
            "--run-id",
            "42",
            "--version",
            version,
        ]);
        fs::write(p.path("image/image.tar"), b"qualified image bytes").expect("image");
        p.xtask(&[
            "image-handoff",
            "seal",
            p.path("image").to_str().expect("path"),
            "--source-sha",
            SOURCE,
            "--run-id",
            "42",
            "--image-id",
            IMAGE,
        ]);
        p.adapter("gh", GH);
        p.adapter("docker", DOCKER);
        p.adapter("timeout", "#!/usr/bin/env bash\nshift\nexec \"$@\"\n");
        p
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn xtask(&self, args: &[&str]) {
        let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(args)
            .output()
            .expect("xtask");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn adapter(&self, name: &str, content: &str) {
        let path = self.path(&format!("bin/{name}"));
        fs::write(&path, content).expect("adapter");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    fn run(&self, fault: &str, current: &str) -> Output {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.github/scripts/publish-release.sh");
        if fault.starts_with("recovery") {
            for entry in fs::read_dir(self.path("dist")).expect("files") {
                let entry = entry.expect("entry");
                if entry.file_name() != "release.json" {
                    fs::copy(entry.path(), self.path("remote").join(entry.file_name()))
                        .expect("existing remote");
                }
            }
            if fault == "recovery-missing" {
                fs::remove_file(self.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
                    .expect("missing public asset");
            }
            if fault == "recovery-corrupt" {
                fs::write(
                    self.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"),
                    b"previously published different bytes",
                )
                .expect("mutate published");
            }
        }
        Command::new("bash")
            .arg(script)
            .arg(env!("CARGO_BIN_EXE_xtask"))
            .arg(self.path("dist"))
            .arg(self.path("image"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.path("bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env("FAKE_ROOT", self.dir.path())
            .env("FAULT", fault)
            .env("CURRENT_VERSION", current)
            .env("TAG", format!("v{}", self.version))
            .env("VERSION", &self.version)
            .env("GITHUB_SHA", SOURCE)
            .env("ARTIFACT_RUN_ID", "42")
            .env("GITHUB_RUN_ID", "43")
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("GITHUB_REPOSITORY", "owner/repo")
            .env("GH_REPO", "owner/repo")
            .env("RUNNER_TEMP", self.path("tmp"))
            .env("GH_TOKEN", "test")
            .env(
                "ALLOW_PUBLISHED_RECOVERY",
                if fault.starts_with("recovery") {
                    "true"
                } else {
                    "false"
                },
            )
            .output()
            .expect("publisher")
    }
    fn trace(&self) -> String {
        fs::read_to_string(self.path("trace")).unwrap_or_default()
    }
    fn diagnostics(&self, output: &Output) -> String {
        format!(
            "status: {}\nstdout: {}\nstderr: {}\ntrace:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            self.trace()
        )
    }
}
const GH: &str = r#"#!/usr/bin/env python3
import os,sys,json,pathlib,shutil
r=pathlib.Path(os.environ['FAKE_ROOT']);a=sys.argv[1:]
with (r/'trace').open('a') as f:f.write('gh '+' '.join(a)+'\n')
fault=os.environ.get('FAULT','')
def opt(k):return a[a.index(k)+1]
if a[:2]==['api','repos/owner/repo/releases/latest']:
 if fault=='gh-latest-auth':print('HTTP 401',file=sys.stderr);sys.exit(1)
 if fault=='gh-latest-absent' and not (r/'github-latest').exists():print('gh: Not Found (HTTP 404)',file=sys.stderr);sys.exit(1)
 tag=(r/'github-latest').read_text() if (r/'github-latest').exists() else ('v2.2.0' if fault=='gh-latest-newer' else 'v2.0.0')
 print(json.dumps({'tag_name':tag,'tagName':tag,'draft':False,'isDraft':False,'prerelease':False,'isPrerelease':False}))
elif a[:2]==['release','view']:
 print(json.dumps({'tagName':os.environ['TAG'],'isDraft':not ((r/'published').exists() or fault.startswith('recovery')),'isPrerelease':'-' in os.environ['VERSION'],'body':'Reviewed notes','id':12,'assets':[{'name':p.name,'size':p.stat().st_size,'state':'uploaded'} for p in (r/'remote').iterdir()]}))
elif a[:2]==['release','upload']:
 if fault=='upload':sys.exit(1)
 for s in a[3:]:
  p=pathlib.Path(s)
  if p.is_file():shutil.copyfile(p,r/'remote'/p.name)
elif a[:2]==['release','download']:
 d=pathlib.Path(opt('--dir'));d.mkdir(parents=True,exist_ok=True)
 for p in (r/'remote').iterdir():shutil.copyfile(p,d/p.name)
 if fault=='corrupt':
  p=next(p for p in d.iterdir() if p.name.endswith('.tar.gz'));p.write_bytes(b'corrupted download')
elif a[:2]==['release','edit']:
 if '--latest=true' in a and fault!='gh-latest-noop':(r/'github-latest').write_text(os.environ['TAG'])
 if '--draft=false' in a or ('--draft' in a and opt('--draft')=='false'):(r/'published').write_text('yes')
else:
 print('unsupported fake gh '+repr(a),file=sys.stderr);sys.exit(2)
"#;
const DOCKER: &str = r#"#!/usr/bin/env python3
import os,sys,json,pathlib
r=pathlib.Path(os.environ['FAKE_ROOT']);a=sys.argv[1:]
with (r/'trace').open('a') as f:f.write('docker '+' '.join(a)+'\n')
b='sha256:'+'b'*64;c='sha256:'+'c'*64
if a[:2]==['image','inspect']:
 if any('org.opencontainers.image.version' in v for v in a):print('9.9.9' if os.environ.get('FAULT')=='candidate-version' else os.environ['VERSION'])
 elif any('org.opencontainers.image.revision' in v for v in a):print('d'*40 if os.environ.get('FAULT')=='candidate-revision' else os.environ['GITHUB_SHA'])
 else:print(b)
elif a[:3]==['buildx','imagetools','inspect']:
 ref=next((v for v in a[3:] if v.startswith('ghcr.io/')),a[-1])
 if os.environ.get('FAULT')=='registry-auth':print('ERROR: unauthorized: authentication required',file=sys.stderr);sys.exit(1)
 if os.environ.get('FAULT')=='registry-dns':print('ERROR: proxy.invalid: not found',file=sys.stderr);sys.exit(1)
 if os.environ.get('FAULT')=='docker-latest-absent' and ref.endswith(':latest') and not (r/'latest-pushed').exists():print('ERROR: '+ref+': not found',file=sys.stderr);sys.exit(1)
 if os.environ.get('FAULT')=='version-conflict' and ref.endswith(':'+os.environ['TAG']):print(c);sys.exit(0)
 if ':latest' in ref or '@'+c in ref or (os.environ.get('FAULT')=='recovery-idempotent' and '@'+b in ref):
  promoted=(r/'latest-pushed').exists() or os.environ.get('FAULT')=='recovery-idempotent';digest=b if promoted else c
  if any('.Image' in v for v in a):print(json.dumps({'config':{'Labels':{} if os.environ.get('FAULT')=='missing-label' else {'org.opencontainers.image.version':os.environ.get('CURRENT_VERSION','2.0.0')}}}))
  else:print(digest)
 elif (r/('pushed-'+ref.split(':')[-1])).exists():print(b)
 else:print('ERROR: '+ref+': not found',file=sys.stderr);sys.exit(1)
elif a and a[0]=='push':
 tag=a[-1].split(':')[-1]
 if tag=='latest' and os.environ.get('FAULT')=='latest-push':sys.exit(1)
 (r/('pushed-'+tag)).write_text(b)
 if tag=='latest':(r/'latest-pushed').write_text(b)
elif a and a[0]=='run':print('memory_mcp '+os.environ.get('CURRENT_VERSION','2.0.0'))
elif a and a[0] in ['tag','load','pull','rm']:pass
else:print('unsupported fake docker '+repr(a),file=sys.stderr);sys.exit(2)
"#;
#[test]
fn publication_verifies_downloaded_bytes_before_publishing_and_switches_latest_last() {
    let p = Publisher::new("2.1.0");
    let result = p.run("", "2.0.0");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let t = p.trace();
    let upload = t.find("gh release upload").expect("upload");
    let download = t.find("gh release download").expect("readback");
    let publish = t.find("gh release edit").expect("publish");
    let latest = t
        .find("docker push ghcr.io/owner/repo:latest")
        .expect("latest");
    assert!(upload < download && download < publish && publish < latest);
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
            .expect("uploaded bytes"),
        b"qualified archive bytes"
    );
}
#[test]
fn upload_failure_keeps_draft_and_latest_unchanged() {
    let p = Publisher::new("2.1.0");
    let r = p.run("upload", "2.0.0");
    assert!(!r.status.success());
    let t = p.trace();
    assert!(!t.contains("gh release edit"));
    assert!(!p.path("latest-pushed").exists());
}
#[test]
fn corrupted_remote_download_keeps_draft_and_latest_unchanged() {
    let p = Publisher::new("2.1.0");
    let r = p.run("corrupt", "2.0.0");
    assert!(!r.status.success());
    assert!(p.trace().contains("gh release download"));
    assert!(!p.trace().contains("gh release edit"));
    assert!(!p.path("latest-pushed").exists());
}
#[test]
fn older_release_does_not_replace_newer_latest() {
    let p = Publisher::new("2.1.0");
    let r = p.run("", "2.2.0");
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(p.trace().contains("gh release edit"));
    assert!(!p.path("latest-pushed").exists());
}
#[test]
fn prerelease_does_not_replace_stable_latest() {
    let p = Publisher::new("2.1.0-rc.1");
    let r = p.run("", "2.0.0");
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(p.trace().contains("gh release edit"));
    assert!(!p.path("latest-pushed").exists());
}

#[test]
fn registry_auth_failure_is_not_treated_as_missing_image() {
    let p = Publisher::new("2.1.0");
    let r = p.run("registry-auth", "2.0.0");
    assert!(!r.status.success());
    assert!(p.trace().contains("imagetools inspect"));
    assert!(!p.trace().contains("docker push"));
    assert!(!p.path("published").exists());
}
#[test]
fn immutable_version_conflict_is_refused() {
    let p = Publisher::new("2.1.0");
    let r = p.run("version-conflict", "2.0.0");
    assert!(!r.status.success());
    assert!(p.trace().contains("ghcr.io/owner/repo:v2.1.0"));
    assert!(!p.path("published").exists());
    assert!(!p.path("latest-pushed").exists());
}
#[test]
fn same_latest_version_with_other_digest_is_refused() {
    let p = Publisher::new("2.1.0");
    let r = p.run("", "2.1.0");
    assert!(!r.status.success());
    assert!(p.trace().contains("imagetools inspect"));
    assert!(!p.path("latest-pushed").exists());
}
#[test]
fn legacy_image_version_is_read_from_exact_digest() {
    let p = Publisher::new("2.1.0");
    let r = p.run("missing-label", "2.0.0");
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let t = p.trace();
    assert!(t.contains("docker pull"));
    assert!(t.contains("docker run"));
    assert!(t.contains("@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"));
    assert!(p.path("latest-pushed").exists());
}
#[test]
fn published_recovery_verifies_existing_assets_without_replacing_them() {
    let p = Publisher::new("2.1.0");
    let r = p.run("recovery", "2.0.0");
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(p.trace().contains("gh release download"));
    assert!(!p.trace().contains("gh release upload"));
}
#[test]
fn published_recovery_rejects_different_existing_bytes() {
    let p = Publisher::new("2.1.0");
    let r = p.run("recovery-corrupt", "2.0.0");
    assert!(!r.status.success());
    assert!(p.trace().contains("gh release download"));
    assert!(!p.trace().contains("gh release upload"));
    assert!(!p.path("latest-pushed").exists());
}

#[test]
fn candidate_image_version_must_match_qualified_version() {
    let p = Publisher::new("2.1.0");
    let r = p.run("candidate-version", "2.0.0");
    assert!(!r.status.success());
    let t = p.trace();
    assert!(t.contains("org.opencontainers.image.version"));
    assert!(!t.contains("docker push"));
    assert!(!t.contains("gh release upload"));
}
#[test]
fn candidate_image_revision_must_match_qualified_commit() {
    let p = Publisher::new("2.1.0");
    let r = p.run("candidate-revision", "2.0.0");
    assert!(!r.status.success());
    let t = p.trace();
    assert!(t.contains("org.opencontainers.image.revision"));
    assert!(!t.contains("docker push"));
    assert!(!t.contains("gh release upload"));
}
#[test]
fn recovery_of_already_current_image_repairs_github_latest_without_repush() {
    let p = Publisher::new("2.1.0");
    let r = p.run("recovery-idempotent", "2.1.0");
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let t = p.trace();
    assert!(t.contains("gh release download"));
    assert!(t.contains("--latest=true"));
    assert!(!t.contains("--latest=false"), "{}", p.diagnostics(&r));
    assert!(!p.path("latest-pushed").exists());
    assert!(!t.contains("gh release upload"));
}

#[test]
fn newer_github_latest_is_preserved_when_docker_channel_can_advance() {
    let p = Publisher::new("2.1.0");
    let r = p.run("gh-latest-newer", "2.0.0");
    let details = p.diagnostics(&r);
    assert!(r.status.success(), "{details}");
    let t = p.trace();
    assert!(
        t.contains("gh api repos/owner/repo/releases/latest"),
        "{details}"
    );
    assert!(!t.contains("--latest=true"), "{details}");
    assert!(p.path("latest-pushed").exists(), "{details}");
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
            .expect("published archive"),
        b"qualified archive bytes"
    );
}

#[test]
fn github_latest_auth_failure_does_not_mutate_latest_channel() {
    let p = Publisher::new("2.1.0");
    let r = p.run("gh-latest-auth", "2.0.0");
    let details = p.diagnostics(&r);
    assert!(!r.status.success(), "{details}");
    let t = p.trace();
    assert!(
        t.contains("gh api repos/owner/repo/releases/latest"),
        "{details}"
    );
    assert!(!t.contains("--latest=true"), "{details}");
    assert!(!t.contains("--latest=false"), "{details}");
}

#[test]
fn missing_github_latest_allows_first_stable_release_promotion() {
    let p = Publisher::new("2.1.0");
    let r = p.run("gh-latest-absent", "2.0.0");
    let details = p.diagnostics(&r);
    assert!(r.status.success(), "{details}");
    let t = p.trace();
    assert!(
        t.contains("gh api repos/owner/repo/releases/latest"),
        "{details}"
    );
    assert!(t.contains("--latest=true"), "{details}");
    assert!(p.path("latest-pushed").exists(), "{details}");
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
            .expect("published archive"),
        b"qualified archive bytes"
    );
}

#[test]
fn missing_docker_latest_allows_first_stable_image_promotion() {
    let p = Publisher::new("2.1.0");
    let r = p.run("docker-latest-absent", "2.0.0");
    let details = p.diagnostics(&r);
    assert!(r.status.success(), "{details}");
    assert!(p.path("latest-pushed").exists(), "{details}");
    assert!(p.trace().contains("--latest=true"), "{details}");
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
            .expect("published archive"),
        b"qualified archive bytes"
    );
}

#[test]
fn registry_dns_not_found_is_not_treated_as_missing_image() {
    let p = Publisher::new("2.1.0");
    let r = p.run("registry-dns", "2.0.0");
    let details = p.diagnostics(&r);
    assert!(!r.status.success(), "{details}");
    let t = p.trace();
    assert!(t.contains("imagetools inspect"), "{details}");
    assert!(!t.contains("docker push"), "{details}");
    assert!(!t.contains("gh release upload"), "{details}");
    assert!(!p.path("published").exists(), "{details}");
}

#[test]
fn github_latest_success_requires_remote_readback() {
    let p = Publisher::new("2.1.0");
    let r = p.run("gh-latest-noop", "2.0.0");
    assert!(!r.status.success(), "{}", p.diagnostics(&r));
    assert!(p.path("published").exists());
    assert!(p.path("latest-pushed").exists());
    assert!(!p.path("github-latest").exists());
}

#[test]
fn final_docker_push_failure_preserves_previous_github_latest() {
    let p = Publisher::new("2.1.0");
    let r = p.run("latest-push", "2.0.0");
    assert!(!r.status.success(), "{}", p.diagnostics(&r));
    assert!(p.path("published").exists());
    assert!(!p.path("latest-pushed").exists());
    assert!(!p.path("github-latest").exists());
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
            .expect("public asset"),
        b"qualified archive bytes"
    );
}

#[test]
fn published_recovery_fills_missing_asset_and_preserves_existing_bytes() {
    let p = Publisher::new("2.1.0");
    let r = p.run("recovery-missing", "2.0.0");
    assert!(r.status.success(), "{}", p.diagnostics(&r));
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-x86_64-unknown-linux-gnu.tar.gz"))
            .expect("repaired asset"),
        b"qualified archive bytes"
    );
    assert_eq!(
        fs::read(p.path("remote/memory_mcp-aarch64-apple-darwin.tar.gz")).expect("preserved asset"),
        b"qualified archive bytes"
    );
    assert_eq!(
        fs::read_to_string(p.path("github-latest")).expect("latest"),
        "v2.1.0"
    );
    assert!(!p.trace().contains("--clobber"));
}
