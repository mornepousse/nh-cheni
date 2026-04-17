//! Build script — capture git metadata at compile time.
//!
//! Emits `GIT_SHORT_HASH` so the binary can display
//! `cheni 0.1.0-alpha (abc1234)` in --version output.

fn main() {
    // Récupérer le hash court du dernier commit
    let git_output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();

    let short_hash = match git_output {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout);
            raw.trim().to_string()
        }
        _ => "unknown".to_string(),
    };

    println!("cargo:rustc-env=GIT_SHORT_HASH={}", short_hash);

    // Recompiler seulement si le HEAD change
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
}
