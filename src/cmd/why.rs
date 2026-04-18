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

    // Group by (category, file) — categories come from the top-level
    // directory under the flake (modules/<cat>, home/, hosts/, ...).
    let mut by_category: std::collections::BTreeMap<String, std::collections::BTreeMap<PathBuf, Vec<&Match>>> =
        std::collections::BTreeMap::new();
    for m in &matches {
        let relative = m.file.strip_prefix(&nix_config.flake_dir)
            .unwrap_or(m.file.as_path());
        let category = categorize(relative);
        by_category
            .entry(category)
            .or_default()
            .entry(m.file.clone())
            .or_default()
            .push(m);
    }

    for (category, files) in &by_category {
        println!("  {}", category.bold().cyan());
        for (file, file_matches) in files {
            let relative = file.strip_prefix(&nix_config.flake_dir)
                .unwrap_or(file.as_path());
            println!("    {}", relative.display().to_string().bold());
            for m in file_matches {
                println!(
                    "      {} {}",
                    format!("{:>3}:", m.line_num).dimmed(),
                    highlight(&m.line, package),
                );
            }
        }
        println!();
    }

    let file_count: usize = by_category.values().map(|f| f.len()).sum();
    let match_count = matches.len();
    println!(
        "{}",
        format!(
            "{} match(es) in {} file(s) across {} categor{}",
            match_count,
            file_count,
            by_category.len(),
            if by_category.len() == 1 { "y" } else { "ies" }
        )
        .dimmed()
    );

    Ok(())
}

/// Map a path (relative to the flake root) to a human-readable category.
/// `modules/apps/foo.nix` → "apps", `home/zsh.nix` → "home (user-level)", etc.
/// Files sitting at the flake root (e.g. flake.nix) are grouped as "root".
fn categorize(rel: &Path) -> String {
    // A file at the root has exactly one component (the filename itself).
    if rel.components().count() <= 1 {
        return "root".to_string();
    }
    let mut comps = rel.components();
    let first = comps.next().and_then(|c| c.as_os_str().to_str()).unwrap_or("");
    match first {
        "modules" => {
            let cat = comps
                .next()
                .and_then(|c| c.as_os_str().to_str())
                .unwrap_or("modules");
            format!("modules/{}", cat)
        }
        "home" => "home (user-level)".to_string(),
        "hosts" => {
            let host = comps
                .next()
                .and_then(|c| c.as_os_str().to_str())
                .unwrap_or("hosts");
            format!("hosts/{}", host)
        }
        other => other.to_string(),
    }
}

/// Render `line` with all occurrences of `needle` painted green+bold.
fn highlight(line: &str, needle: &str) -> String {
    if needle.is_empty() {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let lower_line = line.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let mut start = 0;
    while let Some(pos) = lower_line[start..].find(&lower_needle) {
        let abs = start + pos;
        out.push_str(&line[start..abs]);
        out.push_str(&line[abs..abs + needle.len()].green().bold().to_string());
        start = abs + needle.len();
    }
    out.push_str(&line[start..]);
    out
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
