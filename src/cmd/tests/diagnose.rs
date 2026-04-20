use super::*;

#[test]
fn empty_log_yields_no_findings() {
    assert!(find_issues("").is_empty());
}

#[test]
fn unrelated_log_yields_no_findings() {
    let log = "Configuration built successfully.\nActivating...\nDone.";
    assert!(find_issues(log).is_empty());
}

#[test]
fn detects_aes_generic_kernel_issue() {
    let log = "root module: aes_generic\n\
               modprobe: FATAL: Module aes_generic not found in directory /nix/store/.../7.0.0\n\
               error: Failed to build linux-7.0-modules-shrunk.drv";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("aes_generic"));
}

#[test]
fn detects_hash_mismatch() {
    let log = "error: hash mismatch in fixed-output derivation \
               '/nix/store/...drv':\n  expected: sha256-AAA=\n  got: sha256-BBB=";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("hash mismatch"));
}

#[test]
fn detects_disk_full() {
    let log = "error: writing to file: No space left on device";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("disk full"));
}

#[test]
fn detects_missing_flake_attribute() {
    let log = "error: flake 'gitlab:foo/bar' does not provide attribute \
               'packages.x86_64-linux.default'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("attribute"));
}

#[test]
fn detects_infinite_recursion() {
    let log = "error: infinite recursion encountered\n  \
               at /nix/store/.../default.nix:12:5";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("infinite recursion"));
}

#[test]
fn multiple_issues_in_one_log_all_reported() {
    let log = "error: hash mismatch in fixed-output derivation '/nix/store/...drv'\n\
               ... later ...\n\
               error: writing to file: No space left on device";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 2);
}

#[test]
fn match_is_case_insensitive() {
    let log = "NO SPACE LEFT ON DEVICE somewhere in loud output";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
}

#[test]
fn duplicate_pattern_in_log_reports_once() {
    // A pattern can appear many times in a long log (e.g. retries).
    // We only care that at least one occurrence is surfaced.
    let log = "aes_generic not found\naes_generic not found\naes_generic not found";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
}

#[test]
fn detects_unfree_package_refusal() {
    let log = "error: Package 'steam-1.0.0.82' has an unfree license ('unfree'), \
               refusing to evaluate.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("unfree"));
}

#[test]
fn detects_broken_package_error() {
    let log = "error: Package 'discord-0.0.85' in /nix/store/.../default.nix:123 \
               is marked as broken, refusing to evaluate.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("broken"));
}

#[test]
fn detects_package_collision() {
    let log = "error: collision between `/nix/store/AAA-foo-1.0/bin/foo' \
               and `/nix/store/BBB-bar-2.0/bin/foo'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("collision"));
}

#[test]
fn detects_pure_eval_absolute_path_refusal() {
    let log = "error: access to absolute path '/home/user/secrets.nix' \
               is forbidden in pure eval mode (use '--impure' to override)";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("pure eval mode"));
}

#[test]
fn detects_file_not_in_git_tree() {
    let log = "warning: Git tree '/home/user/nixos-config' is dirty\n\
               error: path '/nix/store/...-source/modules/new.nix' \
               does not exist in the flake";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("tracked by git"));
}

#[test]
fn detects_cached_eval_failure() {
    // The Nix eval cache can replay a stale failure even when the real
    // underlying error has been fixed. The marker is the word "cached"
    // in front of "failure of attribute".
    let log = "error: cached failure of attribute \
               'nixosConfigurations.morthinkpad.config.system.build.toplevel'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("eval-cache"));
}

#[test]
fn detects_tls_failure_on_substituter() {
    let log = "error: unable to download 'https://cache.nixos.org/nar/...': \
               SSL peer certificate or SSH remote key was not OK (60) \
               SSL certificate problem: self signed certificate in certificate chain";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("TLS failure"));
}

#[test]
fn detects_undefined_variable() {
    let log = "error:\n\
               … while calling the 'seq' builtin\n\
               \n\
               at /nix/store/.../default.nix:12:5:\n\
               \n\
               error: undefined variable 'pkgs-unstable'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("undefined variable"));
}

#[test]
fn detects_type_coercion_error() {
    let log = "error: cannot coerce a function to a string\n\
               at /nix/store/.../home.nix:42:5";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("type mismatch"));
}

#[test]
fn detects_disabled_experimental_feature() {
    // The "--extra-experimental-features" hint in the error text itself
    // wouldn't be useful to match against — the stable signature is
    // the "experimental Nix feature ... is disabled" phrasing.
    let log = "error: experimental Nix feature 'nix-command' is disabled; \
               add '--extra-experimental-features nix-command' to enable it";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("experimental feature"));
}

#[test]
fn detects_home_manager_conflict() {
    let log = "Existing file '/home/jdoe/.config/git/config' is in the way of \
               '/nix/store/...-home-manager-files/.config/git/config'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("home-manager"));
}

#[test]
fn detects_github_rate_limit() {
    let log = "error: unable to download \
               'https://api.github.com/repos/NixOS/nixpkgs': \
               HTTP error 403 — API rate limit exceeded for 203.0.113.1";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("GitHub API rate limit"));
}

#[test]
fn detects_oom_kill_exit_code_137() {
    let log = "error: builder for '/nix/store/...-libreoffice-7.6.4.drv' \
               failed with exit code 137";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("OOM killer"));
}

#[test]
fn detects_dns_resolution_failure() {
    let log = "error: unable to download \
               'https://github.com/foo/bar/archive/abc.tar.gz': \
               Could not resolve host: github.com \
               (Temporary failure in name resolution)";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("DNS resolution"));
}

#[test]
fn detects_syntax_error() {
    // Nix parser error output has a fairly stable shape.
    let log = "error: syntax error, unexpected end of file, expecting '}'\n\
               at /nix/store/.../modules/desktop/hyprland.nix:42:5";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("syntax error"));
}

#[test]
fn detects_option_type_mismatch() {
    let log = "error: A definition for option \
               `environment.systemPackages.[definition 1-entry 5]' \
               is not of type `package'. Definition values:\n\
               - In `/etc/nixos/configuration.nix': \"firefox\"";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("wrong type"));
}

#[test]
fn detects_systemd_unit_failure_at_activation() {
    let log = "setting up /etc...\n\
               reloading user units for mae...\n\
               Job for networkmanager.service failed because the control \
               process exited with error code.\n\
               Failed to start Network Manager.\n\
               See 'systemctl status networkmanager.service' for details.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("systemd"));
}

#[test]
fn detects_malformed_flake_url() {
    let log = "error: cannot parse flake reference \
               'github:owner/repo/'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("malformed"));
}

#[test]
fn detects_private_repo_auth_failure() {
    let log = "error: program 'git' failed with exit code 128\n\
               remote: Repository not found or authentication failed\n\
               fatal: Authentication failed for 'https://github.com/private/repo.git/'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("private repository"));
}

#[test]
fn detects_bootloader_install_failure() {
    let log = "installing the boot loader...\n\
               error: failed to install the bootloader.\n\
               See 'journalctl -u boot.mount' for details.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("bootloader"));
}

#[test]
fn detects_eval_time_memory_failure() {
    // Distinct from exit code 137 — this fires BEFORE any build starts.
    let log = "error: while evaluating the attribute \
               'nixosConfigurations.big-config.config.system.build.toplevel':\n\
               std::bad_alloc: cannot allocate memory";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("evaluation"));
}

#[test]
fn detects_untrusted_substituter() {
    let log = "warning: ignoring untrusted substituter \
               'https://my-cache.example.com/', you are not a trusted user. \
               Run 'man nix.conf' for more information.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("cache"));
}

#[test]
fn detects_activation_refusing_to_overwrite() {
    // NixOS activation (not home-manager) can also refuse in-place clobber.
    let log = "activating the configuration...\n\
               refusing to overwrite '/etc/my-service.conf' \
               (owned by a previous manual install)";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("overwrite"));
}

#[test]
fn detects_option_used_but_not_defined() {
    let log = "error:\n\
               The option `hardware.graphics.enable' is used but not defined.\n\
               \n\
               Did you mean one of:\n\
                 hardware.opengl.enable";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("declaring module"));
}

#[test]
fn detects_nar_hash_mismatch_on_flake_input() {
    // Distinct signature from the existing `hash mismatch in fixed-output
    // derivation` pattern — this one is flake-lock, not fetch-derivation.
    let log = "error: NAR hash mismatch in input \
               'github:NixOS/nixpkgs/nixos-unstable':\n\
               wanted sha256-AAAA==\n\
               got    sha256-BBBB==";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("narHash"));
}

#[test]
fn detects_shallow_dependency_failure() {
    let log = "building '/nix/store/abc.drv'...\n\
               ... real error buried earlier ...\n\
               error: 1 dependencies couldn't be built: \
               /nix/store/def-nixos-system-host.drv";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("upstream dependency"));
}

#[test]
fn detects_network_access_inside_sandbox() {
    let log = "error: builder for '/nix/store/xyz.drv' failed \
               with exit code 1;\n\
               last 10 log lines:\n\
               > curl: (6) Could not resolve host: example.com\n\
               > access to network is forbidden";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("sandbox"));
}

#[test]
fn detects_fd_exhaustion() {
    let log = "error: opening file '/nix/store/...-foo.json': \
               Too many open files";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("file-descriptor"));
}
