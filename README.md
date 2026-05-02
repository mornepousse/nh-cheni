# nh-cheni

Personal fork of [nh](https://github.com/nix-community/nh) (Yet Another
Nix Helper) by harrael. Adds NixOS-management tooling on top of upstream
nh while preserving the user-facing `nh` command.

## Status

**Personal use only.** This fork tracks upstream nh and adds tooling
that is specific to harrael's NixOS workflow (pins, freezes, version
cache, timeline, events, check, doctor, bug-report, self-update). Issues
and pull requests are not accepted; please use
[upstream nh](https://github.com/nix-community/nh) if you want to
contribute or report bugs.

The fork lives at `gitlab.com/harrael/nh-cheni`. The previous
**wrapper-era cheni** (a thin Rust CLI that shelled out to nh) is
preserved at the tag `wrapper-archive-v0.8.5` and remains buildable
via `nix build gitlab:harrael/nh-cheni/wrapper-archive-v0.8.5`.

## Install

In your flake:

```nix
{
  inputs.nh-cheni.url = "gitlab:harrael/nh-cheni";
  inputs.nh-cheni.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { nh-cheni, ... }: {
    nixosConfigurations.<host> = {
      modules = [
        { environment.systemPackages = [ nh-cheni.packages.x86_64-linux.default ]; }
      ];
    };
  };
}
```

The installed binary is `nh` (so `nh os switch ...` keeps working
identically to upstream nh, with the cheni-extension subcommands
available alongside). The Nix-store path identifies the fork as
`nh-cheni-<version>`.

## License

[EUPL-1.2](./LICENSE) — inherited from upstream nh.
