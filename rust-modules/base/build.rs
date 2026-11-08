//! Provide host SDL/GL dependencies for surface and heartbeat FFI, including dependent crates.
//! This does not imply that tests create a window or call native drawing functions.
//! ARM32 returns before host discovery/link directives; the Makefile owns its native link.
#[path = "../build_support/host_link.rs"]
mod host_link;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_support/host_link.rs");
    host_link::emit(None, &["SDL2", "GL"]);
}
