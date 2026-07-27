#![forbid(unsafe_code)]

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=bpf/scheduler.bpf.c");
    println!("cargo:rerun-if-changed=bpf/tcp.bpf.c");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/main");
    println!("cargo:rerun-if-env-changed=SYNESTHESIA_GIT_COMMIT");
    if let Some(commit) = build_commit() {
        println!("cargo:rustc-env=SYNESTHESIA_GIT_COMMIT={commit}");
    }

    if env::var_os("CARGO_FEATURE_EBPF").is_none()
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
    {
        return;
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"))
        .join("scheduler.bpf.o");
    compile_bpf(Path::new("bpf/scheduler.bpf.c"), &output);
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR")).join("tcp.bpf.o");
    compile_bpf(Path::new("bpf/tcp.bpf.c"), &output);
}

fn build_commit() -> Option<String> {
    if let Ok(value) = env::var("SYNESTHESIA_GIT_COMMIT") {
        let value = value.trim();
        if !value.is_empty() && value.chars().all(|character| character.is_ascii_hexdigit()) {
            return Some(value.to_owned());
        }
    }
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    (commit.len() == 40
        && commit
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then(|| commit.to_owned())
}

fn compile_bpf(source: &Path, output: &Path) {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR");
    let debug_prefix = format!("-fdebug-prefix-map={manifest_dir}=/src/synesthesia");
    let status = Command::new("clang")
        .args(["-target", "bpf", "-O2", "-g", "-Wall", "-Werror"])
        .arg(debug_prefix)
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(output)
        .status()
        .unwrap_or_else(|error| {
            panic!("the `ebpf` feature needs Clang with the BPF target: {error}")
        });

    assert!(
        status.success(),
        "Clang could not compile {} into {}",
        source.display(),
        output.display()
    );
}
