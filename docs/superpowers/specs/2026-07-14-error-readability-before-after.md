# Switch/activation error readability — before / after (live capture)

Real captures from the dell5430 host, 2026-07-14, that motivated and then
validated the `error_clarify` feature (cheni-layer 0.2.0). Kept verbatim as the
durable record behind
[`2026-07-14-switch-error-readability-design.md`](./2026-07-14-switch-error-readability-design.md).

Both captures are the *same class* of outcome: `switch-to-configuration` exits 4
= "activation applied, but ≥1 unit failed to (re)start". The system switched
fine; one unit failed. Only the presentation differs.

## BEFORE — rendered by the old nh (no clarifier)

`nh os test` with `flatpak-setup.service` failing (offline: `Could not resolve
hostname dl.flathub.org`). The one actionable line is buried under ~40 lines of
routine systemd churn, the exit code is raw, and the `Location:` points into
nh's own source:

```
> Activating configuration
Error:
   0: Activation (test) failed
   1: Activating configuration (exit status ExitStatus(Exited(4)))
      stderr:
      Checking switch inhibitors... done
      stopping the following units: avahi-daemon.service, avahi-daemon.socket, cups-browsed.service, cups.service, cups.socket, dlm.service, ensure-printers.service, fwupd.service, kmod-static-nodes.service, logrotate-checkconf.service, NetworkManager.service, nscd.service, polkit.service, power-profiles-daemon.service, rpc-statd-notify.service, rpc-statd.service, rpcbind.service, rpcbind.socket, rtkit-daemon.service, smartd.service, systemd-binfmt.service, systemd-modules-load.service, systemd-oomd.service, systemd-oomd.socket, systemd-sysctl.service, systemd-timesyncd.service, systemd-vconsole-setup.service, trackpad-monitor.service, udisks2.service, upower.service
      NOT restarting the following changed units: bluetooth.service, greetd.service, post-boot.service, systemd-backlight@backlight:intel_backlight.service, systemd-backlight@leds:dell::kbd_backlight.service, ...
      activating the configuration...
      restarting systemd...
      reloading user units for mae...
      stopping the following user units: dconf.service, gcr-ssh-agent.service, ... (25+ units)
      starting the following user units: dconf.service, gcr-ssh-agent.socket, ... (20+ units)
      restarting the following units: home-manager-mae.service, nix-daemon.service, sshd.service, systemd-journald.service, systemd-resolved.service, systemd-udevd.service, wpa_supplicant.service
      starting the following units: avahi-daemon.socket, cups-browsed.service, ... (25+ units)
      the following new units were started: mandb.service, NetworkManager-dispatcher.service, sysinit-reactivation.target, systemd-coredump@..., systemd-hostnamed.service, systemd-tmpfiles-resetup.service
      warning: the following units failed: flatpak-setup.service


Location:
   crates/nh-core/src/command.rs:907
```

Three defects: (1) `Location:` is nh's own source, not the problem; (2) raw
`Exited(4)` with no meaning; (3) the real signal (`the following units failed:
flatpak-setup.service`) buried in churn, cause reachable only via a manual
`journalctl`.

## AFTER — rendered by nh-cheni 4.4.1+cheni.0.2.0 (error_clarify)

Same exit-4 outcome, deterministic repro via a throwaway failing unit
(`systemd.services.tripwire-demo.script = "echo 'error: …' >&2; exit 1"`):

```
> Activating configuration
⚠ Switch appliqué — la génération est active.
  Mais 1 service a raté son démarrage :
    tripwire-demo.service
      cause : error: simulated demo failure (réseau ?)
      → journalctl -u tripwire-demo.service
  (exit 4 de switch-to-configuration = activé, mais des units ont raté)
```

The failed unit is front and center, the cause is auto-enriched from
`journalctl`, the investigation command is given, exit 4 is explained, and the
color_eyre `Location:` + streamed churn are gone from the final message.

Non-activation failures (e.g. a Nix build error) still fall through to the
default color_eyre report unchanged — verified live: `nh os build` on a bad
flake printed `Error: … Location: crates/nh-core/src/command.rs:1032`, no
clarified block. Selective firing confirmed in both directions.
