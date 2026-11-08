use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    let vendor = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../vendor");
    let manifest_path = vendor.join("ottplay-core.manifest.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(
        manifest["name"], "ottplay-shared-core",
        "Unexpected shared-core distribution"
    );
    for name in ["ottplay-core.js", "ottplay-core.LICENSE.txt"] {
        let path = vendor.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(path).expect("Missing canonical shared-core artifact");
        assert_eq!(
            manifest["artifacts"][name]["bytes"].as_u64(),
            Some(bytes.len() as u64),
            "Shared-core artifact size differs from its distribution receipt: {name}"
        );
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            manifest["artifacts"][name]["sha256"].as_str(),
            Some(digest.as_str()),
            "Shared-core artifact differs from its distribution receipt: {name}"
        );
    }
}
