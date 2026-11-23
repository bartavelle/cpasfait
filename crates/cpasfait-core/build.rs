//! Embeds a Google Drive OAuth client (a "Desktop app" client JSON from the
//! Cloud console), so that users of this build can run `cpasfait-gdrive
//! login` without one. Taken from `$CPASFAIT_GDRIVE_CLIENT` (a path), else
//! `gdrive-client.json` at the workspace root; nothing is embedded if neither
//! exists.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=CPASFAIT_GDRIVE_CLIENT");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("gdrive_client.json");
    let src = match std::env::var_os("CPASFAIT_GDRIVE_CLIENT") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
            .join("../../gdrive-client.json"),
    };
    println!("cargo:rerun-if-changed={}", src.display());
    let data = if std::env::var_os("CARGO_FEATURE_GDRIVE").is_some() && src.exists() {
        let data = std::fs::read_to_string(&src).unwrap_or_else(|e| panic!("reading {}: {e}", src.display()));
        assert!(
            data.contains("\"client_id\"") && data.contains("\"client_secret\""),
            "{} is not an OAuth client JSON file",
            src.display()
        );
        data
    } else {
        String::new()
    };
    std::fs::write(&out, data).expect("writing OUT_DIR");
}
