use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=frontend/admin.ts");
    println!("cargo:rerun-if-changed=bun.lock");
    println!("cargo:rerun-if-changed=static/admin.js");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("admin.js");
    // Bun is both the package manager and the bundler; a machine without it
    // (or without installed dependencies) builds from the committed bundle.
    let bun_ready = PathBuf::from("node_modules/typescript").is_dir()
        && Command::new("bun")
            .arg("--version")
            .output()
            .is_ok_and(|version| version.status.success());
    if bun_ready {
        let typecheck = Command::new("bun")
            .args(["x", "tsc", "--noEmit"])
            .status()
            .expect("could not run TypeScript");
        assert!(
            typecheck.success(),
            "frontend/admin.ts failed type checking"
        );
        let status = Command::new("bun")
            .arg("build")
            .arg("frontend/admin.ts")
            .arg("--format=iife")
            .arg("--minify")
            .arg("--target=browser")
            .arg(format!("--outfile={}", output.display()))
            .status()
            .expect("could not run bun build");
        assert!(status.success(), "could not bundle frontend/admin.ts");
    } else {
        println!("cargo:warning=using the prebuilt admin JavaScript bundle");
        fs::copy("static/admin.js", output).expect("could not copy prebuilt admin JavaScript");
    }
}
