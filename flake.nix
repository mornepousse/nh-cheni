{
  description = "cheni - Granular package updates for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      # Version read from the checked-in ./VERSION file — single source
      # of truth shared with build.rs. Works identically on git+https,
      # gitlab:/github: tarball fetches, and direct `nix build .` from a
      # dirty tree: the file is always present in the source snapshot.
      # The lib.fileContents call strips the trailing newline for us.
      cheniVersion = pkgs.lib.fileContents ./VERSION;

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
