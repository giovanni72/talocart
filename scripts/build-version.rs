//! Compile-time identity. Git is optional for source archives and never used at runtime.
use std::{
    collections::BTreeSet,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug)]
pub struct Identity {
    pub commit: String,
    pub dirty: bool,
    pub tagged: bool,
    pub display: String,
    pub fingerprint: String,
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let result = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn identity(root: &Path, version: &str, sources: Option<&[String]>) -> Identity {
    // An archive nested inside another checkout must not inherit its parent's identity.
    let own_git = git(root, &["rev-parse", "--show-toplevel"])
        .is_some_and(|top| Path::new(&top).canonicalize().ok() == root.canonicalize().ok());
    let commit = own_git
        .then(|| git(root, &["rev-parse", "HEAD"]))
        .flatten()
        .unwrap_or_else(|| "unknown".into());
    let mut args = vec!["status", "--porcelain", "--untracked-files=normal"];
    scope(&mut args, sources);
    let status = own_git.then(|| git(root, &args)).flatten();
    let dirty = status.as_ref().is_some_and(|value| !value.is_empty());
    let tag = format!("v{version}");
    let tagged = status.is_some()
        && !dirty
        && git(root, &["tag", "--points-at", "HEAD", "--list", &tag]).as_deref() == Some(&tag)
        && git(root, &["cat-file", "-t", &format!("refs/tags/{tag}")]).as_deref() == Some("tag");
    let short = commit.get(..12).unwrap_or(&commit);
    let display = format!(
        "{version}{}+{}{}",
        if tagged { "" } else { "-dev" },
        short,
        if dirty { ".dirty" } else { "" }
    );
    Identity {
        commit,
        dirty,
        tagged,
        display,
        fingerprint: own_git
            .then(|| fingerprint(root, sources))
            .flatten()
            .unwrap_or_else(|| "unknown".into()),
    }
}

fn scope<'a>(args: &mut Vec<&'a str>, sources: Option<&'a [String]>) {
    if let Some(sources) = sources {
        args.push("--");
        if sources.is_empty() {
            args.push(":(exclude)**");
        } else {
            args.extend(sources.iter().map(String::as_str));
        }
    }
}

fn fingerprint(root: &Path, sources: Option<&[String]>) -> Option<String> {
    let mut args = vec!["diff", "--binary", "HEAD"];
    scope(&mut args, sources);
    let diff = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !diff.status.success() {
        return None;
    }
    let mut data = diff.stdout;
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z"];
    scope(&mut args, sources);
    for path in git(root, &args)?.split('\0').filter(|p| !p.is_empty()) {
        data.extend_from_slice(path.as_bytes());
        data.push(0);
        data.extend_from_slice(git(root, &["hash-object", "--", path])?.as_bytes());
        data.push(0);
    }
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(&data).ok()?;
    let output = child.wait_with_output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

// Development identity covers executable inputs, not regression fixtures or prose.
// Release/default standalone identity retains the exact whole-checkout contract
// consumed by scripts/release/version.py and package.py.
fn application_source(name: &str) -> bool {
    let name = name.strip_prefix("base/").unwrap_or(name);
    if matches!(name, "Cargo.toml" | "Cargo.lock" | "rust-toolchain.toml")
        || (name.starts_with("LICENSE") && !name.contains('/'))
        || name.starts_with(".cargo/")
        || matches!(
            name,
            "scripts/build-version.rs" | "scripts/locale-catalogues.rs"
        )
    {
        return true;
    }
    let parts: Vec<_> = name.split('/').collect();
    matches!(
        parts.first(),
        Some(&"apps" | &"crates" | &"modules" | &"assets" | &"themes" | &"migrations" | &"seeders")
    ) && !name.ends_with(".md")
        && !parts
            .iter()
            .any(|part| matches!(*part, "tests" | "test-support" | "examples" | "benches"))
}

fn watch(root: &Path, name: &str) {
    // A deleted input must watch its existing parent, never a nonexistent path
    // that would make Cargo rebuild on every invocation.
    let original = root.join(name);
    let relocated = root.join("base").join(name);
    // During an uncommitted relocation HEAD still lists the old paths. Watch
    // their source counterparts, not the repository root (which contains target).
    let input = if !original.exists()
        && root.join("integration/Cargo.toml").is_file()
        && relocated.exists()
    {
        relocated
    } else {
        original
    };
    if let Some(path) = input.ancestors().find(|path| path.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

pub fn emit(root: &Path, version: &str) {
    let normalized = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root = normalized.as_path();
    let root = if !root.join("Cargo.toml").exists()
        && root
            .parent()
            .is_some_and(|p| p.join("integration/Cargo.toml").is_file())
    {
        root.parent().unwrap_or(root)
    } else {
        root
    };
    println!("cargo:rerun-if-env-changed=TALOCART_BUILD_IDENTITY_EXACT");
    let exact = std::env::var("PROFILE").as_deref() != Ok("debug")
        || std::env::var("TALOCART_BUILD_IDENTITY_EXACT").as_deref() == Ok("1");
    let paths = git(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    );
    // The index omits staged deletions. Keep HEAD paths in the identity scope
    // so staging a removed source cannot make a modified build look clean.
    let head_paths = git(root, &["ls-tree", "-r", "--name-only", "-z", "HEAD"]);
    let sources: Vec<String> = paths
        .as_deref()
        .unwrap_or("")
        .split('\0')
        .chain(head_paths.as_deref().unwrap_or("").split('\0'))
        .filter(|name| !name.is_empty() && application_source(name))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let selected = if exact {
        None
    } else {
        Some(sources.as_slice())
    };
    // Track sources, including deletions, and Git refs/index in ordinary and linked worktrees.
    // Ignored runtime data, target/ and node_modules/ must not trigger rebuild loops.
    if let Some(paths) = &paths {
        let mut directories = BTreeSet::new();
        for name in paths.split('\0').filter(|name| !name.is_empty()) {
            if exact || application_source(name) {
                watch(root, name);
                if !exact {
                    let parts: Vec<_> = name.split('/').collect();
                    // Watch source trees for newly created inputs without recursively
                    // watching crate roots (which also contain tests and READMEs).
                    if let Some(index) = parts.iter().position(|part| {
                        matches!(
                            *part,
                            "src"
                                | "templates"
                                | "locales"
                                | "assets"
                                | "themes"
                                | "migrations"
                                | "seeders"
                                | ".cargo"
                        )
                    }) {
                        directories.insert(parts[..=index].join("/"));
                    }
                }
            }
        }
        for directory in directories {
            watch(root, &directory);
        }
    }
    let mut refs = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if exact {
        refs.extend(["index".to_owned(), "refs".to_owned()]);
    } else {
        if let Some(head) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
            refs.push(head);
        }
        // Watch the tags directory to detect creation and deletion of this version's tag.
        refs.push("refs/tags".to_owned());
    }
    for name in refs {
        if let Some(path) = git(root, &["rev-parse", "--git-path", &name]) {
            let path = root.join(path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            } else if name.starts_with("refs/") {
                // Needed for a first tag or a branch whose ref is currently packed.
                if let Some(parent) = path.parent().filter(|parent| parent.exists()) {
                    println!("cargo:rerun-if-changed={}", parent.display());
                }
            }
        }
    }
    for folder in if exact {
        &[
            "base/apps",
            "base/crates",
            "base/assets",
            "base/scripts",
            "base/migrations",
            "base/themes",
            "base/seeders",
            "base/tests",
            "base/docs",
            "base/distribution",
            "base/dev",
            "base/.github",
            "integration",
            "tooling",
            "apps",
            "crates",
            "modules",
            "assets",
            "scripts",
            "migrations",
            "themes",
            "seeders",
            "tests",
            "docs",
            ".github",
        ][..]
    } else {
        &[]
    } {
        let path = root.join(folder);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let info = identity(root, version, selected);
    println!("cargo:rustc-env=TALOCART_BUILD_COMMIT={}", info.commit);
    println!("cargo:rustc-env=TALOCART_BUILD_DIRTY={}", info.dirty);
    println!("cargo:rustc-env=TALOCART_BUILD_TAGGED={}", info.tagged);
    println!("cargo:rustc-env=TALOCART_BUILD_DISPLAY={}", info.display);
    println!(
        "cargo:rustc-env=TALOCART_BUILD_FINGERPRINT={}",
        info.fingerprint
    );
}
