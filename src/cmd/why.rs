//! `cheni why` command.
//!
//! Shows which NixOS module declares a given package, so the user
//! knows where to go to add, remove, or modify it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use crate::nix::config;

/// Tree of grouped matches: category → file → its matches.
type GroupedMatches<'a> = BTreeMap<String, BTreeMap<PathBuf, Vec<&'a Match>>>;

/// Run `cheni why <package>`.
///
/// Searches the user's NixOS config for a package reference and reports
/// the file(s) where it's declared.
pub fn run(package: &str) -> Result<()> {
    let nix_config = config::detect()?;
    println!("{} {}\n", "Searching for".dimmed(), package.bold());

    let mut nix_files = Vec::new();
    collect_nix_files(&nix_config.flake_dir, &mut nix_files);

    let matches = find_matches(&nix_files, package)?;

    if matches.is_empty() {
        print_no_matches(package, &nix_config.flake_dir);
        return Ok(());
    }

    let by_category = group_by_category(&matches, &nix_config.flake_dir);
    print_grouped_matches(&by_category, &nix_config.flake_dir, package);
    print_summary_footer(&by_category, matches.len());
    Ok(())
}

/// Scan every collected file for word-boundary references to `package`.
///
/// The regex anchors on a leading separator (start-of-line or one of
/// `[whitespace, [, (, .]`) so 'python3' doesn't false-match inside
/// 'python311Packages', and lets a trailing dot match attribute access
/// like 'pkgs.python3.withPackages'.
fn find_matches(nix_files: &[PathBuf], package: &str) -> Result<Vec<Match>> {
    let pattern = format!(r"(^|[\s\[(.]){}(\b|\.)", regex::escape(package));
    let re = Regex::new(&pattern)?;
    let mut out = Vec::new();
    for file in nix_files {
        let Ok(content) = std::fs::read_to_string(file) else { continue };
        for (line_num, line) in content.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if re.is_match(line) {
                out.push(Match {
                    file: file.clone(),
                    line_num: line_num + 1,
                    line: line.trim().to_string(),
                });
            }
        }
    }
    Ok(out)
}

fn print_no_matches(package: &str, flake_dir: &Path) {
    println!("{}", "No references found.".dimmed());
    println!(
        "\n  Package '{}' is not referenced in any .nix file under {}.",
        package,
        flake_dir.display()
    );
}

/// Group matches by (category, file). Categories come from the top-level
/// directory under the flake (`modules/<cat>`, `home/`, `hosts/<host>`).
fn group_by_category<'a>(matches: &'a [Match], flake_dir: &Path) -> GroupedMatches<'a> {
    let mut out: GroupedMatches<'a> = BTreeMap::new();
    for m in matches {
        let relative = m.file.strip_prefix(flake_dir).unwrap_or(m.file.as_path());
        out.entry(categorize(relative))
            .or_default()
            .entry(m.file.clone())
            .or_default()
            .push(m);
    }
    out
}

fn print_grouped_matches(by_category: &GroupedMatches<'_>, flake_dir: &Path, package: &str) {
    for (category, files) in by_category {
        println!("{}", category.bold().cyan());
        let file_entries: Vec<_> = files.iter().collect();
        for (fi, (file, file_matches)) in file_entries.iter().enumerate() {
            let last_file = fi == file_entries.len() - 1;
            let file_prefix = if last_file { "└── " } else { "├── " };
            let child_prefix = if last_file { "    " } else { "│   " };
            let relative = file.strip_prefix(flake_dir).unwrap_or(file.as_path());
            println!("{}{}", file_prefix, relative.display().to_string().bold());
            for (mi, m) in file_matches.iter().enumerate() {
                let last_match = mi == file_matches.len() - 1;
                let match_glyph = if last_match { "└── " } else { "├── " };
                let tag = classify_match(&m.line);
                let tag_display = tag
                    .map(|t| format!(" [{}]", t).dimmed().to_string())
                    .unwrap_or_default();
                println!(
                    "{}{}{}{} {}",
                    child_prefix,
                    match_glyph,
                    format!("{:>3}:", m.line_num).dimmed(),
                    tag_display,
                    highlight(&m.line, package),
                );
            }
        }
        println!();
    }
}

/// Lightweight role classification for a matched line.
///
/// Recognises the four shapes that cover almost every "where did this
/// come from?" question in practice:
/// - `enabled` / `disabled` — NixOS option toggles (`.enable = true|false`)
/// - `system` — added to `environment.systemPackages`
/// - `home` — added to home-manager's `home.packages`
///
/// **Limitation**: the classifier operates on a single line. When the
/// `systemPackages` / `home.packages` declaration opens a multi-line
/// `with pkgs; [ ... ]` list, only the line holding the keyword gets
/// tagged — bare package names inside the list come back with no tag.
/// A multi-line-aware version would need a small stack-based
/// pre-pass; deferred until someone actually asks for it.
///
/// Returning `None` is a feature: it's honest when the role is
/// ambiguous rather than guessing.
pub(crate) fn classify_match(line: &str) -> Option<&'static str> {
    let l = line;
    if l.contains(".enable = true") || l.contains(".enable=true") {
        return Some("enabled");
    }
    if l.contains(".enable = false") || l.contains(".enable=false") {
        return Some("disabled");
    }
    if l.contains("environment.systemPackages") || l.contains("systemPackages") {
        return Some("system");
    }
    if l.contains("home.packages") {
        return Some("home");
    }
    None
}

fn print_summary_footer(by_category: &GroupedMatches<'_>, match_count: usize) {
    let file_count: usize = by_category.values().map(|f| f.len()).sum();
    let cat_count = by_category.len();
    println!(
        "{}",
        format!(
            "{} match(es) in {} file(s) across {} categor{}",
            match_count,
            file_count,
            cat_count,
            if cat_count == 1 { "y" } else { "ies" }
        )
        .dimmed()
    );
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

#[cfg(test)]
#[path = "tests/why.rs"]
mod tests;

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
