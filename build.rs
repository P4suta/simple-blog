use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    preserve_openssl_symbols();
    // The whole frontend directory feeds the bundle: a helper module edited on
    // its own must retrigger bundling just like the entry point does.
    println!("cargo:rerun-if-changed=frontend");
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

fn preserve_openssl_symbols() {
    println!("cargo:rerun-if-env-changed=DEP_OPENSSL_INCLUDE");
    if !env::var("TARGET").is_ok_and(|target| target.ends_with("windows-msvc")) {
        return;
    }
    let include = PathBuf::from(
        env::var_os("DEP_OPENSSL_INCLUDE").expect("vendored OpenSSL include metadata"),
    );
    let symbols = include
        .parent()
        .expect("OpenSSL installation")
        .join("lib/ossl_static.pdb");
    if !symbols.is_file() {
        panic!(
            "vendored OpenSSL debug symbols are missing: {}",
            symbols.display()
        );
    }
    // openssl-src removes build/src after installation, but MSVC objects refer
    // to that absolute PDB path. Restore that exact, dependency-specific path;
    // a shared deps/ossl_static.pdb races between check/test feature graphs.
    let original = include
        .parent()
        .and_then(std::path::Path::parent)
        .expect("OpenSSL build root")
        .join("build/src");
    fs::create_dir_all(&original).expect("OpenSSL compiler symbol directory");
    fs::copy(&symbols, original.join("ossl_static.pdb"))
        .expect("preserve the exact OpenSSL compiler symbols");
    println!("cargo:rerun-if-changed={}", symbols.display());
}
