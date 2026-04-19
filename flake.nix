{
  description = "cheni - Granular package updates for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      # Single-source-of-truth version string, mirroring what
      # `git describe --tags --always --dirty` would print:
      #   - `vX.Y.Z` when on an exact release tag
      #   - `vX.Y.Z-N-gHASH` when N commits past the latest tag
      #   - just the short rev when no tag exists yet
      #   - +`-dirty` suffix on a working tree with uncommitted changes
      #
      # In pure Nix we can't reach the git tag history (especially on
      # tarball fetches like `gitlab:` / `github:` shorthand), so we
      # approximate with shortRev/dirtyShortRev. cargo build with .git
      # available will compute the real `git describe` instead — the env
      # var below only acts as a fallback for the Nix sandbox case.
      cheniDescribe =
        self.shortRev or self.dirtyShortRev or "unknown";

      cheni = pkgs.rustPlatform.buildRustPackage {
        pname = "cheni";
        version = cheniDescribe;
        src = ./.;
        env = {
          # Injected into build.rs so `cheni --version` matches the
          # derivation name. The Nix sandbox has no .git/, so without
          # this the binary would fall back to "unknown".
          CHENI_GIT_DESCRIBE = cheniDescribe;
        };

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
