//! Build-time metadata so a running RayRAG identifies the exact source it was
//! built from. The parity ledger advances in slices (`v0.3.4u`, ...) while the
//! crate version only moves on releases, so the startup banner and the version
//! APIs report both plus the git revision and build timestamp.
//!
//! Everything degrades gracefully: the Docker build context has no `.git`
//! directory, so `RAYRAG_BUILD_GIT_REV` can be supplied as an env var and the
//! revision otherwise falls back to `unknown`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RAYRAG_BUILD_GIT_REV");
    println!("cargo:rerun-if-env-changed=RAYRAG_BUILD_TIME");
    // Re-run when the checked-out revision changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    // ...and whenever the compiled sources change, so `built_at` describes the
    // source snapshot a binary was produced from instead of a cached value.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    let rev = std::env::var("RAYRAG_BUILD_GIT_REV")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RAYRAG_BUILD_GIT_REV={rev}");

    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .map(|status| !status.is_empty())
        .unwrap_or(false);
    println!(
        "cargo:rustc-env=RAYRAG_BUILD_GIT_DIRTY={}",
        if dirty { "dirty" } else { "clean" }
    );

    let built_at = std::env::var("RAYRAG_BUILD_TIME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let output = Command::new("date")
                .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
                .output()
                .ok()?;
            let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
            if text.is_empty() { None } else { Some(text) }
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RAYRAG_BUILD_TIME={built_at}");
}
