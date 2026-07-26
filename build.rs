#![forbid(unsafe_code)]

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=bpf/scheduler.bpf.c");
    println!("cargo:rerun-if-changed=bpf/tcp.bpf.c");

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
