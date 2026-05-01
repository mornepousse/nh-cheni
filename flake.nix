{
  description = "cheni — personal fork of nh (NixOS helper) by harrael";

  # Tracks nixos-unstable like the wrapper-era cheni did, so user-side
  # nixos-config that pinned `cheni.inputs.nixpkgs.follows = "nixpkgs"`
  # keeps resolving to the same channel.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

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
      overlays.default = final: _: { cheni = final.callPackage ./package.nix { inherit rev; }; };

      packages = forAllSystems (pkgs: {
        cheni = pkgs.callPackage ./package.nix { inherit rev; };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.cheni;
      });

      checks = self.packages // self.devShells;

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
