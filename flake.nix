{
  description = "cheni - Granular package updates for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      cheni = pkgs.rustPlatform.buildRustPackage {
        pname = "cheni";
        version = "0.1.0";
        src = ./.;

        cargoHash = "sha256-K4oZjnwI8kVt/ot/bwjEQoDcQ3GtiCpn6p843JRL0mM=";

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
