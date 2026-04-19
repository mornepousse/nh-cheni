# Releasing

Version is driven by the checked-in `VERSION` file. Every build path
(cargo dev builds with `.git/`, Nix flake consumers, tarball fetches
via `gitlab:` / `github:` shorthand) reads it from the same place, so
the displayed version never depends on how the source was obtained.

## Cutting a release

1. Update `VERSION` to the new string, e.g. `v0.2.0`.
2. Update `Cargo.toml`'s `version = "..."` to match (strip the leading
   `v` — Cargo demands a bare SemVer literal). `cargo check` will
   update `Cargo.lock`.
3. Commit: `git commit -am "release: v0.2.0"`.
4. Tag: `git tag v0.2.0`.
5. Push both: `git push && git push --tags`.

At that exact commit, `git describe --tags` returns `v0.2.0` verbatim,
matching what `VERSION` contains and what `pkgs.lib.fileContents
./VERSION` gives Nix. The Cargo.toml SemVer is kept in lockstep so
`cargo publish` (if we ever use it) sees the right number too.

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
