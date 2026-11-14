//! Validates the workspace `providers.toml` and generates `OUT_DIR/providers.rs`.
//!
//! Set `CRADLE_PROVIDERS_TOML` to build with a different file.

use std::path::PathBuf;
use std::{env, fs, process};

fn main() {
    println!("cargo::rerun-if-env-changed=CRADLE_PROVIDERS_TOML");

    let path = match env::var_os("CRADLE_PROVIDERS_TOML") {
        Some(p) => PathBuf::from(p),
        None => {
            PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../providers.toml")
        }
    };
    println!("cargo::rerun-if-changed={}", path.display());
    // Lets tests compare the generated table against the source file.
    println!(
        "cargo::rustc-env=CRADLE_PROVIDERS_SOURCE={}",
        path.display()
    );

    let src = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {e}", path.display());
        process::exit(1);
    });

    let specs = cradle_provider_spec::parse_and_validate(&src).unwrap_or_else(|errors| {
        eprintln!("error: invalid {}:", path.display());
        for error in errors.0 {
            eprintln!("  - {error}");
        }
        process::exit(1);
    });

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("providers.rs");
    fs::write(&out, cradle_provider_spec::to_rust_source(&specs)).unwrap_or_else(|e| {
        eprintln!("error: cannot write {}: {e}", out.display());
        process::exit(1);
    });
}
