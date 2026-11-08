//! Host native libraries are provided by their owning crates: base supplies SDL/GL,
//! gfx supplies SDL2_ttf and the SVG archive, platform supplies Linux GLib, and the
//! application supplies Linux Wayland. Mandatory Cargo dependencies carry each native
//! library after the Rust rlibs that use it, including standalone crate test binaries.
//!
//! A no-op for the build that ships: the television binary is linked by the Makefile, not by cargo.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Emit only the native libraries provided by this crate. Other crates inherit them through
/// their mandatory dependencies. Only gfx passes a repository root to build the SVG archive.
pub fn emit(svg_repo: Option<&Path>, libraries: &[&str]) {
    // The television target is the one build cargo does not link, so it is the one that wants
    // none of this. It is identified by ARCHITECTURE rather than by the full triple: the target is
    // `armv7-unknown-linux-gnueabi` (32-bit ARM, `target_arch = "arm"`), while every host that runs
    // this crate's tests is `aarch64` or `x86_64` — including CI's `ubuntu-24.04-arm` runner, whose
    // own arch is `aarch64` and which only ever asks for the ARM32 target explicitly.
    //
    // Read from `CARGO_CFG_TARGET_ARCH` rather than `cfg!`, because a build script is compiled for
    // the HOST: its own `cfg!` would answer for the script, not for the crate being built. Same
    // trap the `CARGO_FEATURE_*` env vars exist for.
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch == "arm" {
        return;
    }

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let mut native = Vec::new();
    let mut packages = Vec::new();
    for &library in libraries {
        let package = match library {
            "SDL2" => "sdl2",
            "SDL2_ttf" => "SDL2_ttf",
            "GL" => "gl",
            "wayland-client" | "glib-2.0" => {
                if target_os != "linux" { continue; }
                library
            }
            _ => panic!("host link: unknown native library {library:?}"),
        };
        native.push(library);
        if !packages.contains(&package) { packages.push(package); }
    }

    if let Some(libdirs) = host_library_search_paths(&target_os, &packages) {
        for libdir in libdirs {
            println!("cargo:rustc-link-search=native={}", libdir.display());
        }
    } else {
        // Not fatal — a system-wide install may need no extra search path. This is not labelled
        // "hostsim": plain host tests now reach the same drawing symbols.
        println!(
            "cargo:warning=host link: could not discover host libraries with the platform package \
             manager; relying on the default linker search path"
        );
    }

    // The gfx owner declares the rasterizer before its system-library dependencies.
    if let Some(repo) = svg_repo {
        compile_svg(repo);
    }

    for library in native {
        if library == "GL" && target_os == "macos" {
            println!("cargo:rustc-link-lib=framework=OpenGL");
        } else {
            println!("cargo:rustc-link-lib=dylib={library}");
        }
    }
}

/// Put the pinned nanosvg headers in `vendor/nanosvg` (gitignored): they are not in the repository.
///
/// `ci/deps.py` returns at once when every header is already there and matches its sha256 in
/// `ci/dependencies.json` (offline, whatever `vendor/downloads` holds). Otherwise it takes them from
/// `vendor/downloads`, then from the machine-wide store under `$PLX_BUILD_CACHE` (default
/// `~/.cache/plxnative`) that every worktree shares, and only then downloads them at the pinned
/// commit, checking each against its sha256 and filling both caches.
/// Different host builds can stage these headers at once: every write is a rename of a
/// private file, so none sees another's partial download. The directory is relative to
/// the repository, not `$HOME`, because some `make check` steps run with an empty home.
fn stage_nanosvg(repo: &Path) {
    // Both exist in every checkout, so `rerun-if-changed` can never name a missing path.
    println!("cargo:rerun-if-changed={}", repo.join("ci/dependencies.json").display());
    println!("cargo:rerun-if-changed={}", repo.join("ci/deps.py").display());
    let status = Command::new("python3")
        .arg(repo.join("ci/deps.py"))
        .args(["stage", "nanosvg", "--into"])
        .arg(repo.join("vendor/nanosvg"))
        .status()
        .unwrap_or_else(|e| panic!("host link: could not run python3 to fetch nanosvg: {e}"));
    assert!(
        status.success(),
        "host link: fetching the pinned nanosvg headers failed ({status}); see ci/deps.py"
    );
}

/// Build `src/svg.c` (the nanosvg rasterizer) for the host.
///
/// On the television the Makefile compiles this alongside `main.c` and `starfish.c` and links all
/// three with the staticlib. The simulator has no Makefile step, so it does the one piece it still
/// needs — `svg.c` is portable C with no webOS in it, and the icon set is rasterized from SVG at
/// runtime, so without it the UI links but has no icons.
///
/// Shelling out to the C compiler rather than taking the `cc` crate as a build-dependency: this is
/// one translation unit with two include paths, and the crate's dependency list is deliberately
/// short — a build-dep would be fetched for the ARM build too, which never runs this function.
fn compile_svg(repo: &Path) {
    stage_nanosvg(repo);
    let src = repo.join("src/svg.c");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let out = out_dir.join("svg.o");
    println!("cargo:rerun-if-changed={}", src.display());

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let st = Command::new(&cc)
        .arg("-c")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg("-O2")
        .arg("-fPIC")
        // Matches the Makefile's include set for this file.
        .arg(format!("-I{}", repo.join("src").display()))
        .arg(format!("-I{}", repo.join("vendor/nanosvg").display()))
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "host link: could not run {cc:?} to compile {}: {e}",
                src.display()
            )
        });
    assert!(
        st.success(),
        "host link: compiling {} failed ({st})",
        src.display()
    );
    // A raw rustc-link-arg object is appended after libc, too late for its stack-canary
    // and other C runtime references on Linux. An ordinary static library is carried
    // with Rust's library dependencies, including when this crate is itself an rlib.
    // Keep normal lazy extraction: no whole-archive/force-load for the shared rasterizer.
    println!("cargo:rerun-if-env-changed=AR");
    let ar = std::env::var("AR").unwrap_or_else(|_| "ar".into());
    let archive = out_dir.join("libplxnative_host_svg.a");
    // Recreate it so an earlier build cannot leave unrelated members in the archive.
    match std::fs::remove_file(&archive) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("host link: could not replace {}: {error}", archive.display()),
    }
    let st = Command::new(&ar)
        .arg("crs")
        .arg(&archive)
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("host link: could not run {ar:?} to archive {}: {e}", out.display()));
    assert!(st.success(), "host link: archiving {} failed ({st})", out.display());
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=plxnative_host_svg");
}

/// Host library directories supplied by the host's package manager.
///
/// A successful pkg-config query may return no `-L` flags because the libraries live on the
/// linker's default path. That is still discovery success, represented by `Some(vec![])`, so a
/// normal Linux install does not emit a misleading discovery warning.
/// Missing metadata for one package does not discard paths discovered for another.
fn host_library_search_paths(target_os: &str, packages: &[&str]) -> Option<Vec<PathBuf>> {
    if packages.is_empty() { return Some(Vec::new()); }
    if target_os == "linux" {
        let mut paths = Vec::new();
        let mut discovered = false;
        for package in packages {
            // Metadata may be absent for a library on the default linker path. Do not let
            // that discard another package's required custom prefix.
            let Ok(status) = Command::new("pkg-config").args(["--exists", package]).status() else {
                continue;
            };
            if !status.success() { continue; }
            // Query the raw variable rather than parsing shell-escaped `-L` flags: prefixes may
            // legitimately contain spaces.
            let Ok(out) = Command::new("pkg-config")
                .args(["--variable=libdir", package])
                .output() else { continue; };
            if !out.status.success() { continue; }
            let Ok(libdir) = String::from_utf8(out.stdout) else { continue; };
            discovered = true;
            let path = PathBuf::from(libdir.trim());
            if !path.as_os_str().is_empty() && !paths.contains(&path) {
                paths.push(path);
            }
        }
        return discovered.then_some(paths);
    }

    if target_os != "macos" {
        return None;
    }
    let out = Command::new("brew").arg("--prefix").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let prefix = String::from_utf8(out.stdout).ok()?;
    let libdir = Path::new(prefix.trim()).join("lib");
    libdir.is_dir().then(|| vec![libdir])
}
