// The `short_rev` helper was folded into `crate::nix::flake::short_hash`
// during the consolidation pass; the behaviour is already covered by
// the existing tests in `src/nix/tests/flake.rs` (`short_hash_truncates_to_twelve`,
// `short_hash_handles_short_input`, `short_hash_survives_non_ascii`).
//
// Leaving this file as a tiny placeholder so the `#[path = "tests/unfreeze.rs"]`
// declaration in `unfreeze.rs` keeps resolving. Add real tests here as
// new logic accrues to `cmd::unfreeze`.
