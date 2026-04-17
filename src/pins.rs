use std::path::PathBuf;
use std::process::Command;

/// Path to the NixOS config
fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join("nixos-config")
}

/// Path to the pins file
fn pins_path() -> PathBuf {
    config_dir().join("package-pins.json")
}

/// Read current pins
fn read_pins() -> Vec<String> {
    let path = pins_path();
    if !path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Save pins
fn write_pins(pins: &[String]) -> bool {
    let content = match serde_json::to_string_pretty(pins) {
        Ok(c) => c,
        Err(_) => return false,
    };
    std::fs::write(pins_path(), content).is_ok()
}

/// Update multiple packages at once
pub fn update_packages(names: &[String]) -> bool {
    let config = config_dir();

    println!("\n=== nixup: updating {} package(s) ===\n", names.len());

    // 1. Add packages to pins
    let mut pins = read_pins();
    let mut added = 0;
    for name in names {
        if !pins.contains(name) {
            pins.push(name.clone());
            added += 1;
        }
    }
    pins.sort();

    if added > 0 {
        if !write_pins(&pins) {
            eprintln!("Error: unable to write package-pins.json");
            wait_for_enter();
            return false;
        }
    }

    for name in names {
        println!("  + {}", name);
    }
    println!();

    // 2. Update nixpkgs-latest only
    println!("[1/2] Updating nixpkgs-latest...");
    let update_result = Command::new("nix")
        .args(["flake", "update", "nixpkgs-latest"])
        .current_dir(&config)
        .status();

    match update_result {
        Ok(status) if status.success() => {
            println!("      OK");
        }
        _ => {
            eprintln!("Error: nix flake update nixpkgs-latest failed");
            wait_for_enter();
            return false;
        }
    }

    // 3. Rebuild
    println!("[2/2] Rebuilding system...");
    let rebuild_result = Command::new("nh")
        .args(["os", "switch", config.to_str().unwrap_or(".")])
        .status();

    match rebuild_result {
        Ok(status) if status.success() => {
            println!("\n{} package(s) updated successfully!", names.len());
            wait_for_enter();
            true
        }
        _ => {
            eprintln!("\nError: rebuild failed");
            // Remove added pins on failure
            let mut pins = read_pins();
            for name in names {
                pins.retain(|p| p != name);
            }
            write_pins(&pins);
            wait_for_enter();
            false
        }
    }
}

/// Wait for the user to press Enter
fn wait_for_enter() {
    println!("\nPress Enter to return to nixup...");
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
}
