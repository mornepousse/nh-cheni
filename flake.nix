{
  description = "nh-cheni — personal fork of nh (NixOS helper) by harrael";

  inputs.nixpkgs.url = "https://channels.nixos.org/nixos-unstable/nixexprs.tar.xz";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      forAllSystems =
        function:
        nixpkgs.lib.genAttrs [
          "x86_64-linux"
          "aarch64-linux"
          "x86_64-darwin"
          "aarch64-darwin"
        ] (system: function nixpkgs.legacyPackages.${system});

      rev = self.shortRev or self.dirtyShortRev or "dirty";
    in
    {
      overlays.default = final: _: { nh-cheni = final.callPackage ./package.nix { inherit rev; }; };

      packages = forAllSystems (pkgs: {
        nh-cheni = pkgs.callPackage ./package.nix { inherit rev; };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.nh-cheni;
      });

      checks = builtins.removeAttrs (self.packages // self.devShells) [ "x86_64-darwin" ];

      devShells = forAllSystems (pkgs: {
        default = import ./shell.nix { inherit pkgs; };
      });

      formatter = forAllSystems (
        pkgs:
        pkgs.writeShellApplication {
          name = "nix3-fmt-wrapper";

          runtimeInputs = [
            pkgs.nixfmt-rfc-style
            pkgs.fd
          ];

          text = ''
            fd "$@" -t f -e nix -x nixfmt -q '{}'
          '';
        }
      );
    };
}
