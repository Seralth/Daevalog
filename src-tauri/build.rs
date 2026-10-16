use std::path::Path;
use std::process::Command;

fn main() {
    version_label();
}

/// `DAEVALOG_VERSION`: the release line from Cargo.toml ("1.0") and, when
/// known, the revision: "1.0 · r250". The revision is the commit count, from
/// `DAEVALOG_REVISION` if set, else from git. A source tree without its git
/// history, or a shallow clone, shows the release line alone.
fn version_label() {
    let line = format!(
        "{}.{}",
        std::env::var("CARGO_PKG_VERSION_MAJOR").unwrap_or_default(),
        std::env::var("CARGO_PKG_VERSION_MINOR").unwrap_or_default()
    );
    println!("cargo:rerun-if-env-changed=DAEVALOG_REVISION");
    let revision = match std::env::var("DAEVALOG_REVISION") {
        Ok(value) => Some(value.trim().trim_start_matches('r').to_string()),
        Err(_) => git_revision(),
    }
    .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()));
    let label = match revision {
        Some(r) => format!("{line} · r{r}"),
        None => line,
    };
    println!("cargo:rustc-env=DAEVALOG_VERSION={label}");
}

/// The commit count of the repository this tree is the top of. A full clone
/// only: a shallow clone counts too few commits, and a tree copied into some
/// other repository would count that one's.
fn git_revision() -> Option<String> {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let root = Path::new(&manifest).parent()?;
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let same_tree = match (Path::new(&top).canonicalize(), root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same_tree {
        return None;
    }
    // A new commit or a checkout moves the count: build again then.
    for watched in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(root, &["rev-parse", "--path-format=absolute", "--git-path", watched]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if git(root, &["rev-parse", "--is-shallow-repository"])? != "false" {
        return None;
    }
    git(root, &["rev-list", "--count", "HEAD"])
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
