use std::fs;

fn main() {
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
                new_toml.push_str(&format!("version = \"{new_version}\""));
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
