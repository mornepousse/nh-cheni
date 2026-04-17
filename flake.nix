{
  description = "nixup - TUI to check for NixOS package updates";

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

        cargoHash = "sha256-4h3huWQSobUTKaGW+22Kj+uEyLnchyRfufySYzvibtI=";

        # reqwest utilise rustls-tls, pas besoin de pkg-config ni openssl
        nativeBuildInputs = [];
        buildInputs = [];

        meta = with pkgs.lib; {
          description = "TUI to check for NixOS package updates";
          homepage = "https://github.com/mae/nixup";
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
