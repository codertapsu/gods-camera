//! Link clang's runtime library on Apple platforms when the model runtime is on.
//!
//! ONNX Runtime's Core ML code asks "is this OS new enough?" through a helper
//! that lives in that library. A C++ program gets it automatically; Rust does
//! not link it, and the build fails with `___isPlatformVersionAtLeast` missing.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if std::env::var_os("CARGO_FEATURE_RUNTIME").is_none() {
        return;
    }
    let name = match target_os.as_str() {
        "macos" => "osx",
        "ios" => "ios",
        _ => return,
    };
    let library = format!("libclang_rt.{name}.a");
    let found = Command::new("xcrun").args(["clang", &format!("--print-file-name={library}")]).output();
    let Ok(found) = found else {
        println!("cargo:warning=xcrun was not found; {library} is not linked");
        return;
    };
    let path = PathBuf::from(String::from_utf8_lossy(&found.stdout).trim());
    match path.parent() {
        Some(folder) if path.is_file() => {
            println!("cargo:rustc-link-search=native={}", folder.display());
            println!("cargo:rustc-link-lib=static=clang_rt.{name}");
        }
        _ => println!("cargo:warning={library} was not found; linking may fail"),
    }
}
