{
  description = "nixup - Granular package updates for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      nixup = pkgs.rustPlatform.buildRustPackage {
        pname = "nixup";
        version = "0.1.0";
        src = ./.;

        cargoHash = "sha256-IurfS7oZwA/cw5rP41Lr99H6GI0Rr1dqWxio2B+TO2s=";

        # reqwest uses rustls-tls, no need for pkg-config or openssl
        nativeBuildInputs = [];
        buildInputs = [];

        meta = with pkgs.lib; {
          description = "Granular package updates for NixOS";
          homepage = "https://gitlab.com/harrael/nixup";
          license = licenses.mit;
          mainProgram = "nixup";
        };
      };
    in
    {
      packages.${system} = {
        default = nixup;
        nixup = nixup;
      };

      overlays.default = final: prev: {
        nixup = self.packages.${system}.nixup;
      };
    };
}
