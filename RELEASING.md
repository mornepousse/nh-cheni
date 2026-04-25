# Releasing

Version is driven by the checked-in `VERSION` file. Every build path
(cargo dev builds with `.git/`, Nix flake consumers, tarball fetches
via `gitlab:` / `github:` shorthand) reads it from the same place, so
the displayed version never depends on how the source was obtained.

## Cutting a release

1. **Pre-release quality gate** — all four must pass on a clean tree:
   ```sh
   cargo build
   cargo clippy --all-targets
   cargo test
   nix flake check
   ```
   The first three catch Rust-level regressions on the host shell.
   `nix flake check` runs the same `cargo test` *inside the Nix
   sandbox*, where PATH is empty by default — that's the only way
   to catch tests that quietly rely on host tools (git, nvd, …)
   not being declared as `nativeCheckInputs` in `flake.nix`. See
   the v0.5.1 → v0.5.2 cycle: 7 tests passed locally and broke the
   sandbox build because git was missing from the check phase. A
   `nix flake check` before tagging would have flagged it before
   the tarball reached the world.
2. Update `VERSION` to the new string, e.g. `v0.2.0`.
3. Update `Cargo.toml`'s `version = "..."` to match (strip the leading
   `v` — Cargo demands a bare SemVer literal). `cargo check` will
   update `Cargo.lock`.
4. Commit: `git commit -am "release: v0.2.0"`.
5. Tag: `git tag v0.2.0`.
6. Push both: `git push && git push --tags`.
7. Sign the auto-archive tarball with minisign (below).
8. Create the GitLab release object and attach the signature.

At that exact commit, `git describe --tags` returns `v0.2.0` verbatim,
matching what `VERSION` contains and what `pkgs.lib.fileContents
./VERSION` gives Nix. The Cargo.toml SemVer is kept in lockstep so
`cargo publish` (if we ever use it) sees the right number too.

## Signing (steps 6–7)

cheni releases are signed with minisign so `cheni self-update` (and
any human who cares) can verify that a downloaded release matches a
trusted private key. The public counterpart is checked in at
`public-keys/cheni-release.pub` and embedded in the cheni binary.

Prerequisites:

- `~/.minisign/cheni.key` — password-protected private key on the
  maintainer workstation (generate once with `minisign -G`; never
  commit this file).
- `glab` authenticated to `gitlab.com` (`glab auth status`).

Procedure (run in a fresh temp dir, e.g. `mktemp -d`):

```sh
TAG=v0.2.0
curl -fsSL "https://gitlab.com/harrael/cheni/-/archive/${TAG}/cheni-${TAG}.tar.gz" \
  -o "cheni-${TAG}.tar.gz"

nix shell nixpkgs#minisign --command minisign \
  -Sm "cheni-${TAG}.tar.gz" \
  -s ~/.minisign/cheni.key \
  -t "cheni ${TAG} release"

# Self-check before uploading — catches a bad key or corrupted fetch.
nix shell nixpkgs#minisign --command minisign \
  -Vm "cheni-${TAG}.tar.gz" \
  -p public-keys/cheni-release.pub

glab release create "${TAG}" "cheni-${TAG}.tar.gz.minisig" \
  --name "${TAG}" \
  --notes "Signed release. Verify: \`minisign -Vm cheni-${TAG}.tar.gz -p public-keys/cheni-release.pub\`"
```

The tarball itself is never uploaded as an asset — GitLab serves it
from the auto-archive endpoint and that's what everyone fetches,
including the signature verification path. Only the `.minisig` needs
to travel as a release asset.

If GitLab ever changes how it generates auto-archives, past signatures
would stop verifying retroactively. If that happens, document it
prominently and publish re-signed tarballs as release assets directly.

## Between releases

Commits after the tag don't need to touch `VERSION`. The dev build
output evolves on its own:

  - `cargo build` → `cheni v0.2.0-5-g37073ac` (5 commits past the tag).
  - `cargo build` on a dirty tree → `cheni v0.2.0-5-g37073ac-dirty`.
  - `nix build .` or `cheni self-update` → `cheni v0.2.0` (whatever
    `VERSION` said at the last release — the tag + commit info
    isn't recoverable from the tarball, that's fine).

## Rationale

Why a `VERSION` file when we already have `Cargo.toml::version` and
git tags?

- **`Cargo.toml::version`** can't contain a `v` prefix or a `-N-g…`
  suffix (Cargo parses it as strict SemVer). It's the "stable" name.
- **Git tags** aren't preserved by tarball fetchers (`gitlab:` ships
  only the tree of one commit, no refs). They're useful to devs but
  invisible inside the Nix sandbox.
- **`VERSION`** is just a file. Every fetcher copies it. Both cargo
  and Nix read it identically. The only cost is remembering to bump
  it at release time — which is step 1 above.
