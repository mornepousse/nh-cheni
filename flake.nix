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
      # derivation name matches what `cheni --version` prints at runtime:
      #   self.revCount   → commit count from git rev-list (patch number)
      #   self.shortRev   → 7-char hash of HEAD (falls back to dirty one)
      # Dirty trees don't have revCount, so we default to 0 and mark as
      # "dirty" so it's obvious in `nix store --references` output.
      cheniVersion =
        if self ? revCount
        then "0.1.${toString self.revCount}-alpha+${self.shortRev}"
        else "0.1.0-alpha+${self.dirtyShortRev or "dirty"}";

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
