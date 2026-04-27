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
    // The real kernel-module-missing signal — modprobe writes exactly this phrase.
    let log = "modprobe: FATAL: module 'aes_generic' not found in directory \
               /nix/store/.../7.0.0\n\
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
    let log = "module 'aes_generic' not found\n\
               module 'aes_generic' not found\n\
               module 'aes_generic' not found";
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

// ── aes_generic resserrement (Gap 1) ────────────────────────────────────────

#[test]
fn aes_generic_does_not_fire_on_pure_test_failure_log() {
    // A log that only mentions a Rust test failure — the word "aes_generic"
    // appears nowhere, so the resserred matcher must stay silent.
    let log = "test result: FAILED. 7 passed; 4 failed; 0 ignored; 0 measured; \
               0 filtered out; finished in 0.01s\n\
               error: test failed, to rerun pass `--test smoke`\n\
               error: Cannot build 'cheni-0.5.8.drv'.\n\
               Reason: builder failed with exit code 101.";
    let hits = find_issues(log);
    // Only the test-panic finding should fire, not aes_generic.
    assert!(
        hits.iter().all(|f| !f.title.contains("aes_generic")),
        "aes_generic finding fired on a pure test-failure log"
    );
}

#[test]
fn aes_generic_does_not_fire_on_bare_word_mention() {
    // Logs that mention the string "aes_generic" but not in a
    // module-not-found context (e.g. a comment in an NixOS config
    // appearing in a build log trace) must not trigger the finding.
    let log = "trace: evaluating 'boot.initrd.availableKernelModules' \
               value contains \"aes_generic\"";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("aes_generic")),
        "aes_generic finding fired on a bare-word mention with no not-found signal"
    );
}

// ── Rust checkPhase test panic (Gap 2) ──────────────────────────────────────

#[test]
fn detects_rust_test_panic_in_checkphase() {
    // Realistic snippet from the v0.5.8 build failure.
    let log = "running 11 tests\n\
               test smoke::version_flag ... ok\n\
               test smoke::help_flag ... ok\n\
               test smoke::hostname_lookup ... FAILED\n\
               \n\
               failures:\n\
               \n\
               ---- smoke::hostname_lookup stdout ----\n\
               thread 'smoke::hostname_lookup' panicked at 'hostname binary not found', \
               tests/smoke.rs:42\n\
               \n\
               test result: FAILED. 7 passed; 4 failed; 0 ignored; 0 measured; \
               0 filtered out; finished in 0.01s\n\
               error: test failed, to rerun pass `--test smoke`\n\
               error: Cannot build 'cheni-0.5.8.drv'.\n\
               Reason: builder failed with exit code 101.";
    let hits = find_issues(log);
    assert!(
        hits.iter().any(|f| f.title.contains("Rust test panicked")),
        "test-panic finding did not fire on a realistic checkPhase failure log"
    );
}

// ── Phase-aware refactor: scoped fixtures ───────────────────────────────────

/// The v0.5.8 failure that motivated the refactor: cheni's own
/// checkPhase panicked on `tests/smoke.rs`, the build aborted, and
/// the rendering of the log triggered a spurious `aes_generic` hint
/// in production.
const LOG_V058_FAILURE: &str = "\
cheni> running 11 tests
cheni> test history_list_exits_zero_and_shows_header ... FAILED
cheni> test doctor_on_minimal_flake_exits_zero ... FAILED
cheni>
cheni> test result: FAILED. 7 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
cheni>
cheni> error: test failed, to rerun pass `--test smoke`
error: Cannot build 'cheni-v0.5.8.drv'.
       Reason: builder failed with exit code 101.
       Output paths:
         cheni-v0.5.8
";

#[test]
fn v058_log_fires_only_test_panic_not_aes_generic() {
    // The real reproduction: even if the word `aes_generic` had
    // appeared somewhere in this log (e.g. an unrelated trace), the
    // Build-scoped finding must not fire — the failing derivation's
    // panic is in checkPhase, not buildPhase.
    let log_with_red_herring = format!(
        "{}\n\
         trace: noting that boot.initrd.availableKernelModules contains aes_generic\n",
        LOG_V058_FAILURE
    );
    let hits = find_issues(&log_with_red_herring);
    let titles: Vec<&str> = hits.iter().map(|f| f.title).collect();
    assert!(
        titles.iter().any(|t| t.contains("Rust test panicked")),
        "expected the Rust-test-panic finding, got {:?}",
        titles
    );
    assert!(
        titles.iter().all(|t| !t.contains("aes_generic")),
        "aes_generic finding must NOT fire on a log whose failure is a test panic; got {:?}",
        titles
    );
}

/// A real kernel-modules-shrunk failure: the literal modprobe error
/// lands inside the failing derivation's buildPhase.
const LOG_AES_GENERIC_REAL: &str = "\
nixos-system> @nix { \"action\": \"setPhase\", \"phase\": \"buildPhase\" }
nixos-system> buildPhase
nixos-system> modprobe: FATAL: module 'aes_generic' not found in directory /nix/store/abc-linux-7.0.0
nixos-system> error: Failed to build linux-7.0-modules-shrunk.drv
error: builder for '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-nixos-system.drv' failed with exit code 1
";

#[test]
fn aes_generic_fires_on_real_kernel_module_log() {
    let hits = find_issues(LOG_AES_GENERIC_REAL);
    assert!(
        hits.iter().any(|f| f.title.contains("aes_generic")),
        "aes_generic finding did not fire on a real modules-shrunk failure"
    );
}

/// A log where the literal string `aes_generic` appears only in an
/// eval-time trace (debug listing of kernel modules), with no
/// modprobe / build-phase context. The Build-scoped finding must
/// stay silent.
const LOG_AES_GENERIC_IN_EVAL_TRACE: &str = "\
evaluating flake outputs...
trace: kernel modules considered: [\"aes\" \"aes_generic\" \"crypto_blkcipher\"]
warning: Git tree '/home/mae/nixos-config' is dirty
";

#[test]
fn aes_generic_silent_in_pure_eval_trace() {
    let hits = find_issues(LOG_AES_GENERIC_IN_EVAL_TRACE);
    assert!(
        hits.iter().all(|f| !f.title.contains("aes_generic")),
        "aes_generic finding fired on an eval-only trace mentioning the word"
    );
}

/// Pre-switch check kicked in during activation. This is the
/// activation-scope canonical case.
const LOG_PRE_SWITCH_REFUSED: &str = "\
building Nix...
building the system configuration...
activating the configuration...
Pre-switch check: refusing to switch the running system because dbus is moving to dbus-broker.
Switching into this system is not recommended; please reboot.
";

#[test]
fn pre_switch_refused_log_fires_activation_findings() {
    let hits = find_issues(LOG_PRE_SWITCH_REFUSED);
    assert!(
        hits.iter().any(|f| f.title.contains("live switch refused")),
        "Pre-switch refused finding did not fire on its canonical log"
    );
}

#[test]
fn failing_derivation_phase_strict_when_anchor_missing() {
    // Architecture test from the spec: with `derivation_a>` carrying
    // the test failure, an `error: Cannot build 'derivation_a.drv'`
    // anchor SHOULD allow the FailingDerivationPhase finding to
    // fire. Without that anchor, it must NOT fire — even though the
    // panic phrase is right there.
    let log_with_anchor = "\
derivation_a> @nix { \"action\": \"setPhase\", \"phase\": \"checkPhase\" }
derivation_a> running 3 tests
derivation_a> test result: FAILED. 0 passed; 3 failed
derivation_b> all good
error: Cannot build '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-derivation_a.drv'.
";
    let hits_with = find_issues(log_with_anchor);
    assert!(
        hits_with.iter().any(|f| f.title.contains("Rust test panicked")),
        "test-panic finding must fire when failing-derivation anchor + checkPhase + nh prefix all line up"
    );

    let log_without_anchor = "\
derivation_a> @nix { \"action\": \"setPhase\", \"phase\": \"checkPhase\" }
derivation_a> running 3 tests
derivation_a> test result: FAILED. 0 passed; 3 failed
derivation_b> all good
";
    let hits_without = find_issues(log_without_anchor);
    assert!(
        hits_without
            .iter()
            .all(|f| !f.title.contains("Rust test panicked")),
        "test-panic finding must NOT fire without an `error: Cannot build` anchor"
    );
}

// ── Helper-level unit tests ─────────────────────────────────────────────────

#[test]
fn split_derivation_prefix_extracts_name_and_rest() {
    let (name, rest) = super::split_derivation_prefix("cheni> running tests").unwrap();
    assert_eq!(name, "cheni");
    assert_eq!(rest, "running tests");
}

#[test]
fn split_derivation_prefix_accepts_dotted_and_dashed_names() {
    let (name, _) = super::split_derivation_prefix("linux-7.0-modules-shrunk> oops").unwrap();
    assert_eq!(name, "linux-7.0-modules-shrunk");
    let (name2, _) = super::split_derivation_prefix("foo.bar_baz+1> body").unwrap();
    assert_eq!(name2, "foo.bar_baz+1");
}

#[test]
fn split_derivation_prefix_rejects_non_prefix_lines() {
    assert!(super::split_derivation_prefix("error: -> something").is_none());
    assert!(super::split_derivation_prefix("plain text without a marker").is_none());
    // A pure-numeric "prefix" (line numbers, pids) must be rejected.
    assert!(super::split_derivation_prefix("12345> not a derivation").is_none());
}

#[test]
fn extract_phase_recognises_each_phase_marker() {
    use super::Phase;
    assert_eq!(super::extract_phase("buildPhase"), Some(Phase::Build));
    assert_eq!(super::extract_phase("checkPhase"), Some(Phase::Check));
    assert_eq!(super::extract_phase("installPhase"), Some(Phase::Install));
    assert_eq!(
        super::extract_phase("activating the configuration..."),
        Some(Phase::Activate)
    );
    assert_eq!(
        super::extract_phase("Pre-switch check: refusing to switch"),
        Some(Phase::Activate)
    );
    assert_eq!(
        super::extract_phase("Updating flake inputs..."),
        Some(Phase::Eval)
    );
    assert_eq!(
        super::extract_phase("trying https://example.com/foo.tar.gz"),
        Some(Phase::Fetch)
    );
    assert_eq!(super::extract_phase("ordinary chatter"), None);
}

#[test]
fn find_failing_derivation_handles_cannot_build_anchor() {
    let log = "lots of noise\n\
               error: Cannot build '/nix/store/aaaabbbbccccddddeeeeffffgggghhhh-cheni-0.5.8.drv'.\n";
    assert_eq!(super::find_failing_derivation(log), Some("cheni-0.5.8"));
}

#[test]
fn find_failing_derivation_handles_builder_for_anchor() {
    let log = "error: builder for '/nix/store/aaaabbbbccccddddeeeeffffgggghhhh-foo.drv' failed";
    assert_eq!(super::find_failing_derivation(log), Some("foo"));
}

#[test]
fn find_failing_derivation_returns_none_on_success_log() {
    assert_eq!(
        super::find_failing_derivation("everything is fine"),
        None
    );
}

#[test]
fn parse_log_context_makes_phase_sticky_per_derivation() {
    let log = "\
foo> @nix { \"phase\": \"checkPhase\" }
foo> checkPhase
foo> running 3 tests
foo> test result: FAILED.
bar> hi from a different derivation
foo> still in foo's checkphase
";
    let ctx = super::parse_log_context(log);
    let foo_lines: Vec<_> = ctx
        .lines
        .iter()
        .filter(|l| l.derivation == Some("foo"))
        .collect();
    assert!(
        foo_lines
            .iter()
            .all(|l| l.phase == Some(super::Phase::Check)),
        "every foo> line should inherit checkPhase: {:?}",
        foo_lines.iter().map(|l| l.phase).collect::<Vec<_>>()
    );
    let bar_phase = ctx
        .lines
        .iter()
        .find(|l| l.derivation == Some("bar"))
        .and_then(|l| l.phase);
    assert_eq!(
        bar_phase, None,
        "bar's stream had no phase marker yet; phase must not leak across derivations"
    );
}

#[test]
fn test_panic_finding_does_not_fire_on_clean_build() {
    let log = "building '/nix/store/abc-cheni-0.5.8.drv'...\n\
               running 11 tests\n\
               test result: ok. 11 passed; 0 failed; 0 ignored\n\
               all tests passed\n\
               /nix/store/xyz-cheni-0.5.8 done.";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("Rust test panicked")),
        "test-panic finding fired on a clean build log"
    );
    // aes_generic must also stay silent on a clean log.
    assert!(
        hits.iter().all(|f| !f.title.contains("aes_generic")),
        "aes_generic finding fired on a clean build log"
    );
}

// ── Findings positive cases manquants ───────────────────────────────────────

#[test]
fn detects_switching_not_recommended() {
    // Second Activate-scoped finding, distinct de "Pre-switch check".
    // Le log minimal : juste la ligne incriminée, pas de phase marker
    // → fallback Global (any_phase_seen = false).
    let log = "Switching into this system is not recommended; please reboot.";
    let hits = find_issues(log);
    assert!(
        hits.iter().any(|f| f.title.contains("live switch not recommended")),
        "live-switch-not-recommended finding did not fire"
    );
}

#[test]
fn switching_not_recommended_silent_in_build_phase() {
    // Scoped Activate → doit être silencieux si on est clairement en
    // buildPhase et que le matcher ne s'y trouve pas.
    // Note : le texte exact "Switching into this system" est lui-même
    // un marker de phase Activate — on teste donc avec un log dont la
    // phase est explicitement buildPhase et dont aucune ligne ne
    // contient la phrase incriminée.
    let log = "\
nixos-system> buildPhase
nixos-system> compile step 1
nixos-system> compile step 2
error: builder for '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-nixos-system.drv' failed
";
    let hits = find_issues(log);
    assert!(
        hits.iter()
            .all(|f| !f.title.contains("live switch not recommended")),
        "live-switch-not-recommended fired inside buildPhase"
    );
}

#[test]
fn detects_insecure_package() {
    let log =
        "error: Package 'qtwebengine-5.15.19' in /nix/store/.../default.nix:5 \
         is marked as insecure, refusing to evaluate.\n\
         Known CVEs: CVE-2024-1234";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].title.contains("insecure"),
        "insecure-package finding did not fire"
    );
}

#[test]
fn insecure_package_silent_in_build_phase() {
    // "is marked as insecure" is Eval-scoped — must not fire if the
    // log context is clearly buildPhase.
    let log = "\
pkg> buildPhase
pkg> error: is marked as insecure — hypothetical build log line
error: builder for '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-pkg.drv' failed with exit code 1
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("insecure")),
        "insecure-package finding fired inside buildPhase (wrong scope)"
    );
}

// ── Tests négatifs manquants pour findings scoped ───────────────────────────

#[test]
fn oom_kill_137_silent_in_activate_phase() {
    // exit code 137 is Build-scoped. In an activation context
    // (boot loader install), the string must not trigger the OOM hint.
    let log = "\
activating the configuration...
installing the boot loader...
error: failed with exit code 137
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("OOM killer")),
        "OOM-killer finding fired inside Activate phase"
    );
}

#[test]
fn auth_failed_silent_in_build_phase() {
    // "Authentication failed for" is Fetch-scoped — must not fire when
    // the log is clearly in a buildPhase context.
    let log = "\
mypkg> buildPhase
mypkg> fatal: Authentication failed for 'https://github.com/example/repo.git'
error: builder for '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-mypkg.drv' failed
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("private repository")),
        "private-repo-auth finding fired inside buildPhase (wrong scope)"
    );
}

#[test]
fn failed_to_start_silent_in_build_phase() {
    // "Failed to start" is Activate-scoped — must not fire if we're
    // clearly in a build context.
    let log = "\
mypkg> buildPhase
mypkg> make: Failed to start subprocess
error: builder for '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-mypkg.drv' failed
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("systemd")),
        "systemd-failed-to-start finding fired inside buildPhase"
    );
}

#[test]
fn hash_mismatch_silent_in_build_phase() {
    // "hash mismatch in fixed-output derivation" is Fetch-scoped.
    // A build-phase log that mentions "hash mismatch" in a different
    // context (e.g. a test checking hashes) must not trigger the finding.
    let log = "\
mypkg> buildPhase
mypkg> self-test: hash mismatch in fixed-output derivation (expected)
error: builder for '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-mypkg.drv' failed
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("hash mismatch")),
        "hash-mismatch finding fired inside buildPhase (wrong scope)"
    );
}

#[test]
fn rust_test_panic_silent_wrong_phase_with_anchor() {
    // FailingDerivationPhase(Check): even if we have the anchor AND
    // nh prefixes, the finding must NOT fire if the matching line is
    // in buildPhase, not checkPhase.
    let log = "\
mypkg> buildPhase
mypkg> test result: FAILED. 0 passed; 1 failed
error: Cannot build '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-mypkg.drv'.
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("Rust test panicked")),
        "test-panic finding fired in buildPhase instead of checkPhase"
    );
}

// ── Edge cases du parser ─────────────────────────────────────────────────────

#[test]
fn single_line_log_no_panic() {
    // Un log d'une seule ligne ne doit pas provoquer de panic.
    let result = find_issues("error: undefined variable 'pkgs'");
    // Avec fallback (any_phase_seen = false) → Phase(Eval) dégradé en Global.
    // On vérifie surtout qu'il n'y a pas de panic.
    let _ = result;
}

#[test]
fn non_ascii_in_log_no_panic() {
    // Des caractères non-ASCII au milieu d'une ligne ne doivent pas
    // provoquer de panic (boundary UTF-8 dans split_derivation_prefix).
    let log = "mypkg> buildPhase\n\
               mypkg> erreur : fichier introuvable — chemin « /nix/store/… »\n\
               mypkg> No space left on device\n";
    let hits = find_issues(log);
    // "No space left on device" est Global → doit fire.
    assert!(hits.iter().any(|f| f.title.contains("disk full")));
}

#[test]
fn large_log_no_oom_or_panic() {
    // Un log de 15 000 lignes ne doit pas OOM ni paniquer.
    // On génère des lignes de ~80 chars avec contenu varié.
    let mut log = String::with_capacity(15_000 * 80);
    for i in 0..15_000_usize {
        log.push_str(&format!("mypkg> building object file number {i} out of 14999\n"));
    }
    log.push_str("error: Cannot build '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-mypkg.drv'.\n");
    // Aucun matcher ne devrait fire ici, et surtout pas de panic.
    let hits = find_issues(&log);
    let _ = hits; // le test réussit si on arrive ici sans OOM
}

#[test]
fn malformed_nh_prefix_no_bracket_no_crash() {
    // Un `>` collé sans espace après, ou avec tab, ne doit pas paniquer
    // et doit être traité comme une ligne sans prefix.
    let log = ">no space after bracket\n\
               mypkg>\tTab after bracket\n\
               plain line\n";
    // split_derivation_prefix doit rejeter ">no space after bracket"
    // (nom vide) et "mypkg>\t..." (tab n'est pas ' ').
    let ctx = super::parse_log_context(log);
    // La première ligne a un nom vide → pas de derivation reconnue.
    // La deuxième a un tab → rest commence au tab, derivation = "mypkg".
    // On vérifie surtout l'absence de panic.
    let _ = ctx;
}

#[test]
fn multi_failure_picks_first_anchor() {
    // Quand deux `error: Cannot build` apparaissent, find_failing_derivation
    // retourne le premier (comportement documenté : on prend le plus proche
    // dans le log). Le deuxième ne doit pas écraser le premier.
    let log = "\
foo> checkPhase
foo> test result: FAILED. 0 passed; 1 failed
error: Cannot build '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-foo.drv'.
error: Cannot build '/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-bar.drv'.
";
    let failing = super::find_failing_derivation(log);
    assert_eq!(failing, Some("foo"), "expected first anchor to win");
}

#[test]
fn conflicting_phase_markers_sticky_per_derivation() {
    // eval → fetch → eval : la phase doit être sticky par derivation.
    // Les marqueurs globaux (sans prefix) ne doivent pas contaminer
    // les derivation-prefixed streams.
    let log = "\
evaluating flake outputs...
foo> buildPhase
foo> compile step
downloading 'https://cache.nixos.org/nar/abc.nar'
foo> still in foo's buildPhase
";
    let ctx = super::parse_log_context(log);
    // La ligne "foo> still in foo's buildPhase" doit rester en Build.
    let foo_still = ctx
        .lines
        .iter()
        .find(|l| l.derivation == Some("foo") && l.text.contains("still"));
    assert_eq!(
        foo_still.and_then(|l| l.phase),
        Some(super::Phase::Build),
        "foo's phase should remain Build despite an intervening global Fetch marker"
    );
    // La ligne de download (global) doit être Fetch.
    let dl = ctx
        .lines
        .iter()
        .find(|l| l.derivation.is_none() && l.text.contains("downloading"));
    assert_eq!(
        dl.and_then(|l| l.phase),
        Some(super::Phase::Fetch),
        "global download line should carry Fetch phase"
    );
}

// ── Backward compat : fallback sans phase markers ────────────────────────────

#[test]
fn phase_scoped_finding_fires_when_log_has_no_phase_markers() {
    // Fallback 1 : log sans AUCUN phase marker → Phase(X) se dégrade
    // en Global. Une ligne brute avec "undefined variable" doit fire
    // même si on n'est pas en Eval explicite.
    let log = "error: undefined variable 'pkgs'";
    let hits = find_issues(log);
    assert!(
        hits.iter().any(|f| f.title.contains("undefined variable")),
        "Phase(Eval) finding must fire when no phase markers are present (fallback)"
    );
}

#[test]
fn failing_derivation_phase_fires_in_degraded_mode_no_nh_prefixes() {
    // Fallback 2 : log sans préfixes nh mais avec un anchor
    // `error: Cannot build` → FailingDerivationPhase doit fire car
    // any_deriv_prefix = false (mode dégradé, on accepte toutes les lignes).
    let log = "\
@nix { \"action\": \"setPhase\", \"phase\": \"checkPhase\" }
checkPhase
running 3 tests
test result: FAILED. 0 passed; 3 failed
error: Cannot build '/nix/store/aaabbbcccdddeeefffggghhhiiijjjkk-mypkg.drv'.
";
    let hits = find_issues(log);
    assert!(
        hits.iter().any(|f| f.title.contains("Rust test panicked")),
        "FailingDerivationPhase finding must fire in degraded mode (no nh prefixes)"
    );
}

#[test]
fn failing_derivation_phase_silent_without_anchor_even_degraded() {
    // Même en mode dégradé (pas de préfixes nh), l'absence d'anchor
    // `error: Cannot build` doit garder FailingDerivation* silencieux.
    let log = "\
checkPhase
running 3 tests
test result: FAILED. 0 passed; 3 failed
";
    let hits = find_issues(log);
    assert!(
        hits.iter().all(|f| !f.title.contains("Rust test panicked")),
        "FailingDerivationPhase must NOT fire without anchor, even without nh prefixes"
    );
}

#[test]
fn global_log_no_prefixes_no_phases_behaves_as_legacy() {
    // Un log totalement plat (style `nix build` brut, sans préfixes
    // ni phase markers) → les findings Global doivent fire, les
    // findings Phase(X) aussi grâce au fallback, les findings
    // FailingDerivation* uniquement s'il y a un anchor.
    let log = "error: No space left on device\n\
               error: hash mismatch in fixed-output derivation '/nix/store/abc.drv'";
    let hits = find_issues(log);
    // Global finding.
    assert!(hits.iter().any(|f| f.title.contains("disk full")));
    // Phase(Fetch) finding — fallback dégradé.
    assert!(hits.iter().any(|f| f.title.contains("hash mismatch")));
}
