# Security

This document describes the trust model of cheni, how to verify a
release, and what to do if you suspect a compromise.

## Trust model

cheni is a local CLI run by the user on their own NixOS machine. The
threat we defend against is **code tampering of the cheni binary
itself** — a malicious release making its way onto a user's system
through either:

1. A compromise of `gitlab.com/harrael/cheni` (the repository that
   ships the source).
2. A MITM between the user's `nix flake update cheni` call and GitLab.
3. A downstream mirror or cache serving a modified tarball.

We do **not** defend against:

- An attacker who is already the local user on the machine (they can
  just replace `cheni` in `$PATH` without needing to forge anything).
- Bugs in cheni itself that produce unintended system changes. Those
  are regular software bugs and are fixed with regular code review.
- A compromise of the maintainer's workstation that exfiltrates the
  private signing key. If this happens, **we cannot help you from
  within cheni** — see "Compromise response" below.

## How releases are signed

Each tagged release tarball published by GitLab is signed with
[minisign](https://jedisct1.github.io/minisign/) using a private key
held by the maintainer. The signature is attached to the release as
a `.minisig` asset.

- **Public key**: checked into the repository at
  [`public-keys/cheni-release.pub`](public-keys/cheni-release.pub)
  and embedded in every cheni binary at compile time.
- **Fingerprint**: `358A303A12B2640B`
- **Key bytes**: `RWQLZLISOjCKNePLEuZBR02kr04oqqlpyr3eEjjhJ564pRdh6NjadVTo`

The detailed release procedure is in
[`RELEASING.md`](RELEASING.md#signing-steps-67).

## Verifying a release

### Automatically — through cheni itself

`cheni self-update` checks the signature of the new release against
the embedded public key **before** calling `nh os switch`. A
verification failure aborts the rebuild. This happens every time,
without a flag to enable.

`cheni verify` does the same check on demand, without touching the
system — useful for pre-upgrade audits or after-the-fact confirmation:

```
cheni verify                 # verify the installed version
cheni verify --tag v0.2.0    # verify an arbitrary tag
```

Both require your `flake.nix` to pin `cheni` to a tagged release:

```nix
inputs.cheni.url = "gitlab:harrael/cheni/v0.2.0";
```

Without a tag pin, `flake.lock` carries no `ref` and the
verification layer cannot know which release to check against. It
refuses by default; the escape hatch is `cheni self-update
--allow-unsigned`, which should be used sparingly and with a reason.

### Manually — without cheni

If you do not yet have cheni installed, or you want to cross-check
independently of any cheni binary:

```
curl -fLO https://gitlab.com/harrael/cheni/-/archive/v0.2.0/cheni-v0.2.0.tar.gz
curl -fLO https://gitlab.com/harrael/cheni/-/releases/v0.2.0/downloads/cheni-v0.2.0.tar.gz.minisig
minisign -Vm cheni-v0.2.0.tar.gz -P RWQLZLISOjCKNePLEuZBR02kr04oqqlpyr3eEjjhJ564pRdh6NjadVTo
```

Exit 0 plus `Signature and comment signature verified` = the tarball
was produced by the holder of the matching private key.

## What a verification failure means

If `cheni self-update` or `cheni verify` reports a verification
failure, treat it as a red flag and investigate before proceeding.
Common causes, in decreasing order of likelihood:

1. **No signed release for that tag yet.** The `.minisig` asset is
   missing on GitLab because it's a tag that predates this signing
   system, or a tag was pushed without going through the release
   procedure. Not a security event, but cheni refuses out of
   caution. Use `--allow-unsigned` only after confirming the tag is
   legitimate (e.g. you created it yourself).
2. **Network issue.** A 5xx from GitLab or a partial download
   surfaces as "signature check failed" because the downloaded
   bytes don't match. Retry.
3. **Key rotation.** The maintainer has rotated the signing key and
   your cheni binary is still pinned to the old key. Upgrade cheni
   manually once through `--allow-unsigned`, cross-checking the new
   public key fingerprint against an out-of-band source (maintainer
   announcement, multiple cheni releases, etc.).
4. **Genuine tampering.** The tarball you received is not the one
   the maintainer signed. Stop. Do not use `--allow-unsigned`. File
   an issue, or contact the maintainer out-of-band.

## Compromise response

If the private signing key is suspected compromised — workstation
theft, accidental leak, etc. — the recovery path is:

1. **Generate a new minisign keypair** on a clean machine.
2. **Publish the new public key** in `public-keys/cheni-release.pub`
   (the old one is moved to `public-keys/archive/` and retained so
   historical releases remain verifiable).
3. **Cut a new release** signed with both the old key (for current
   cheni installations that don't yet know the new key) and the new
   key (establishing trust going forward). This is the transition
   release.
4. **Document the rotation** in a release note and, ideally, via an
   out-of-band channel (blog post, signed commit, mastodon post —
   anything the user can cross-check).
5. **Encourage upgrades.** Any installation that doesn't upgrade
   through the transition release loses verification capability for
   subsequent releases and must rely on `--allow-unsigned` with
   manual cross-checking until they catch up.

## Reporting a security issue

If you believe you've found a security issue in cheni — a bug that
could be exploited, a hole in the signing chain, or something
similar — please **do not** open a public issue. Instead, contact
the maintainer directly:

- Email: see the GitLab profile at
  <https://gitlab.com/harrael>
- Or open a **confidential issue** on GitLab
  (<https://gitlab.com/harrael/cheni/-/issues/new> → toggle
  "Confidential").

Expect a response within a few days. cheni is maintained by a single
person on their spare time, so the turnaround isn't instant, but
security reports are prioritized.

## What's out of scope for signing

The following are intentionally not covered by the release signature
and are worth being aware of:

- **Runtime dependencies fetched by `nix flake update`** — nixpkgs,
  transitive Rust crates, etc. Those have their own integrity
  mechanisms (narHash, Cargo.lock) but cheni doesn't re-verify them
  on top.
- **First-install bootstrap** — the very first install of cheni on a
  new machine relies on whatever brought the binary there (usually
  Nix's narHash over the flake tarball). The minisign chain only
  protects upgrades from that point on.
- **The self-update UX itself** — a sufficiently-old cheni that
  predates the signing system will upgrade without verification.
  Once you've passed through a signed release, every subsequent
  upgrade is checked.
