//! Host link configuration and SVG archive ownership for the gfx layer.
//!
//! This crate provides SDL2_ttf and declares the rasterizer's C symbols, so it alone builds
//! the nanosvg archive. SDL and GL calls inherit their native libraries from mandatory base.
//! The shared helper declares it as an ordinary native static library. Cargo carries it
//! through the gfx rlib to dependent tests and executables without whole-archive flags.
//!
//! On the television `host_link::emit` returns before emitting any link or archiver
//! directives: the ARM32 staticlib is linked by the Makefile instead.

#[path = "../build_support/host_link.rs"]
mod host_link;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_support/host_link.rs");
    host_link::emit(Some(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")), &["SDL2_ttf"]);
}
