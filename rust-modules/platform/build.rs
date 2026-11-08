//! Code generation for the platform layer: the `i18n::msg` localization catalog and the language registry's `Preference`/`Language` enums and the
//! install-identity `Flavor` enum `storage::state` includes. On Linux hosts this crate also
//! provides the GLib native dependency for its LS2 callbacks and dependent application code.
//! SDL/GL come from mandatory base; native linking on ARM32 remains the Makefile's job.
//!
//! Both generators write into this crate's `OUT_DIR` and both are included only from this crate
//! (`i18n/mod.rs`, `storage/state.rs`). The storage helper (`plxnative-storage`) includes
//! `storage/state.rs` by `#[path]` and runs `install_identities` from its own build script, so the
//! client and the helper cannot disagree about which installs exist.
//!
//! The inputs are the only things this script re-runs for: the catalog sources under `locales/`,
//! the packaging manifest of installs, the two generator files, and shared host link configuration.

use std::path::{Path, PathBuf};

#[path = "../build_support/host_link.rs"]
mod host_link;
#[path = "build_support/catalog.rs"]
mod catalog;
#[path = "../build_support/install_identities.rs"]
mod install_identities;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_support/host_link.rs");
    host_link::emit(None, &["glib-2.0"]);
    println!("cargo:rerun-if-changed=build_support/catalog.rs");
    println!("cargo:rerun-if-changed=../build_support/install_identities.rs");
    println!("cargo:rerun-if-changed=../../locales");
    if let Err(mut errors) =
        catalog::build(Path::new("../../locales"), &PathBuf::from(std::env::var_os("OUT_DIR").unwrap()))
    {
        errors.sort();
        for e in &errors {
            eprintln!("{e}");
        }
        panic!("localization catalogs: {} error(s), listed above", errors.len());
    }

    install_identities::emit(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ci/install-identities.json"));
}
