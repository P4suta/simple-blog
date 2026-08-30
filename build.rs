use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=frontend/admin.ts");
    println!("cargo:rerun-if-changed=package-lock.json");
    println!("cargo:rerun-if-changed=static/admin.js");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("admin.js");
    let esbuild = PathBuf::from("node_modules/esbuild/bin/esbuild");
    if esbuild.is_file() {
        let typecheck = Command::new("node")
            .arg("node_modules/typescript/bin/tsc")
            .arg("--noEmit")
            .status()
            .expect("could not run TypeScript");
        assert!(
            typecheck.success(),
            "frontend/admin.ts failed type checking"
        );
        let status = Command::new(esbuild)
            .arg("frontend/admin.ts")
            .arg("--bundle")
            .arg("--minify")
            .arg("--target=es2022")
            .arg("--log-level=warning")
            .arg(format!("--outfile={}", output.display()))
            .status()
            .expect("could not run esbuild");
        assert!(status.success(), "could not bundle frontend/admin.ts");
    } else {
        println!("cargo:warning=using the prebuilt admin JavaScript bundle");
        fs::copy("static/admin.js", output).expect("could not copy prebuilt admin JavaScript");
    }
}
