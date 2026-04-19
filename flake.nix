{
  description = "cheni - Granular package updates for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      # Derive the Nix version from the flake's own git metadata so the
      # derivation name is unique-per-commit rather than a static "0.1.0".
      # Three cases, in order of preference:
      #   1. git tree (revCount set)   → 0.1.{count}-alpha+{shortRev},
      #      mirrors exactly what `cheni --version` prints at runtime.
      #   2. tarball fetch (shortRev only, no revCount) — this is the
      #      GitLab / GitHub flake-input path → 0.1.0-alpha+{shortRev}.
      #   3. dirty local tree                          → +dirty-{hash}.
      # Having the rev in the version helps `nvd` diff output and the
      # /nix/store path carry the identity of the build.
      cheniVersion =
        if self ? revCount then
          "0.1.${toString self.revCount}-alpha+${self.shortRev}"
        else if self ? shortRev then
          "0.1.0-alpha+${self.shortRev}"
        else
          # dirtyShortRev already carries its own "-dirty" suffix
          # (e.g. "835648d-dirty"), so we don't prepend another one.
          "0.1.0-alpha+${self.dirtyShortRev or "unknown"}";

      cheni = pkgs.rustPlatform.buildRustPackage {
        pname = "cheni";
        version = cheniVersion;
        src = ./.;

        # Derive the vendored-deps hash from Cargo.lock directly — no manual
        # bump needed when deps change (cheni relies on this for self-update
        # to keep working after 'cargo add'). Only works while every dep
        # comes from crates.io; add `outputHashes` here if we ever pull a
        # git or local dep.
        cargoLock = {
          lockFile = ./Cargo.lock;
        };

        # reqwest uses rustls-tls, no need for pkg-config or openssl
        nativeBuildInputs = [];
        buildInputs = [];

        meta = with pkgs.lib; {
          description = "Granular package updates for NixOS";
          homepage = "https://gitlab.com/harrael/cheni";
          license = licenses.mit;
          mainProgram = "cheni";
        };
      };
    in
    {
      packages.${system} = {
        default = cheni;
        cheni = cheni;
      };

      overlays.default = final: prev: {
        cheni = self.packages.${system}.cheni;
      };
    };
}
