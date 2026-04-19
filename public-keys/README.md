# Public keys for release signing

This directory holds the public keys used to verify the authenticity of
cheni releases.

## `cheni-release.pub`

Minisign public key used to sign every tarball published as a GitLab
release asset. The same key is embedded in the cheni binary so that
`cheni self-update` can verify a downloaded release before applying it.

**Fingerprint:** `358A303A12B2640B`

### Verifying a release manually

Download the tarball and its `.minisig` companion from the GitLab
release page, then:

```
minisign -Vm cheni-vX.Y.Z.tar.gz -p public-keys/cheni-release.pub
```

Exit 0 + `Signature and comment signature verified` = the tarball was
signed by the holder of the matching private key.

### Key rotation

If this public key ever changes, a new file is added here (never
overwritten) and the old one is moved to `archive/`. Users running an
older cheni are still able to verify releases signed by the previous
key. The transition plan for any rotation is documented in
`RELEASING.md`.

### What this key does NOT protect against

- An attacker who steals the private key. Mitigated by storing the
  private key password-encrypted in `~/.minisign/cheni.key` with
  `chmod 600`.
- Downgrade attacks to older signed versions. `cheni self-update`
  refuses versions older than the currently installed one.
- Compromise of the channel used to distribute the initial cheni
  binary (first install). That bootstrap trust is inherited from
  whatever brought cheni onto the machine originally — typically Nix's
  `narHash` over the tarball fetched from GitLab.
