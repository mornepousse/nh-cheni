//! `cheni why` command.
//!
//! Shows which NixOS module declares a given package, so the user
//! knows where to go to add, remove, or modify it.

use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use crate::nix::config;

/// Run `cheni why <package>`.
///
/// Searches the user's NixOS config for a package reference and reports
/// the file(s) where it's declared.
pub fn run(package: &str) -> Result<()> {
    let nix_config = config::detect()?;

    println!(
        "{} {}\n",
        "Searching for".dimmed(),
        package.bold()
    );

    // Collect all .nix files in the config
    let mut nix_files = Vec::new();
    collect_nix_files(&nix_config.flake_dir, &mut nix_files);

    // Build a regex that matches the package name as a word boundary
    // (avoids matching "python3" inside "python311Packages")
    let pattern = format!(r"(^|[\s\[(.]){}(\b|\.)", regex::escape(package));
    let re = Regex::new(&pattern)?;

    let mut matches = Vec::new();

    for file in &nix_files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (line_num, line) in content.lines().enumerate() {
            // Skip comments
            if line.trim_start().starts_with('#') {
                continue;
            }

            if re.is_match(line) {
                matches.push(Match {
                    file: file.clone(),
                    line_num: line_num + 1,
                    line: line.trim().to_string(),
                });
            }
        }
    }

    if matches.is_empty() {
        println!("{}", "No references found.".dimmed());
        println!(
            "\n  Package '{}' is not referenced in any .nix file under {}.",
            package,
            nix_config.flake_dir.display()
        );
        return Ok(());
    }

    // Group by file
    let mut by_file: std::collections::BTreeMap<PathBuf, Vec<&Match>> = std::collections::BTreeMap::new();
    for m in &matches {
        by_file.entry(m.file.clone()).or_default().push(m);
    }

    for (file, file_matches) in &by_file {
        let relative = file.strip_prefix(&nix_config.flake_dir)
            .unwrap_or(file.as_path());

        println!("  {}", relative.display().to_string().bold());
        for m in file_matches {
            println!(
                "    {} {}",
                format!("{}:", m.line_num).dimmed(),
                m.line,
            );
        }
        println!();
    }

    let file_count = by_file.len();
    let match_count = matches.len();
    println!(
        "{}",
        format!(
            "{} match(es) in {} file(s)",
            match_count, file_count
        ).dimmed()
    );

    Ok(())
}

/// A single match in a .nix file.
struct Match {
    file: PathBuf,
    line_num: usize,
    line: String,
}

/// Recursively collect all .nix files in a directory.
fn collect_nix_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip common non-config directories
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == ".git" || name == "result" || name == "target"
                    || name == "node_modules" || name.starts_with('.') {
                    continue;
                }
            }
            collect_nix_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "nix") {
            out.push(path);
        }
    }
}
