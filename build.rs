// Build script: build frontend and bump version.
// Allow expect/unwrap here since build scripts should abort on failure.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    // Build frontend if present
    if Path::new("frontend/package.json").exists() {
        println!("cargo:rerun-if-changed=frontend/src");
        println!("cargo:rerun-if-changed=frontend/index.html");
        println!("cargo:rerun-if-changed=frontend/package.json");

        if !Path::new("frontend/node_modules").exists() {
            let status = Command::new("npm")
                .args(["install"])
                .current_dir("frontend")
                .status()
                .expect("failed to run npm install");
            assert!(status.success(), "npm install failed");
        }

        let status = Command::new("npm")
            .args(["run", "build"])
            .current_dir("frontend")
            .status()
            .expect("failed to run npm run build");
        assert!(status.success(), "npm run build failed");
    }

    // Read current version from Cargo.toml and bump the patch number
    let cargo_toml = fs::read_to_string("Cargo.toml").unwrap();
    let mut new_toml = String::new();
    let mut bumped = false;

    for line in cargo_toml.lines() {
        if !bumped && line.starts_with("version = \"") {
            let version_str = line
                .trim_start_matches("version = \"")
                .trim_end_matches('"');
            let parts: Vec<u32> = version_str
                .split('.')
                .filter_map(|s| s.parse().ok())
                .collect();
            if parts.len() == 3 {
                let new_version = format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1);
                let _ = write!(new_toml, "version = \"{new_version}\"");
                bumped = true;
            } else {
                new_toml.push_str(line);
            }
        } else {
            new_toml.push_str(line);
        }
        new_toml.push('\n');
    }

    if bumped {
        fs::write("Cargo.toml", new_toml).unwrap();
    }

    // Always rerun so version bumps on every compile
    println!("cargo:rerun-if-changed=build_trigger");
}
