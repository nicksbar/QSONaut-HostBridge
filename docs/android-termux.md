# Android shell / Termux HostBridge

HostBridge can run as a foreground process from an Android shell such as
Termux. This is useful when the Android device is the network-side host or
when an Android-specific hardware adapter is added later. It does not require
systemd, root, or an Android application package.

## Install

Install [Termux](https://termux.dev/) from a current supported distribution,
open the shell, and run:

```sh
pkg update -y
pkg install -y git
git clone https://github.com/nicksbar/QSONaut-HostBridge "$HOME/QSONaut-HostBridge"
cd "$HOME/QSONaut-HostBridge"
./scripts/install-termux.sh
```

The script installs the user-level Rust and C toolchain, builds the native
Android binary, and copies it to `$PREFIX/bin`. It uses the pinned Rigwright
Git dependency; no sibling checkout is required.

## Configure and run

```sh
qsonaut-hostbridge config set --key android-station --password change-me --bind 0.0.0.0:8765
qsonaut-hostbridge config show
qsonaut-hostbridge devices
qsonaut-hostbridge run
```

The process stays attached to the shell. Use Termux `tmux`, `nohup`, or a
Termux:Boot/foreground-service launcher if it must continue after the shell
closes. `service install` is intentionally unsupported on Android because
Android has no user systemd instance.

## Current Android boundary

The protocol and HostBridge session runtime are Android-buildable Rust. The
current hardware adapter is Linux-specific: it discovers `/dev/serial/by-id`
and ALSA `arecord`/`aplay` devices. A stock Termux install therefore provides
the authenticated network host and control surface, but will normally report
empty radio/audio catalogs.

Android USB radios require an adapter that uses Android USB Host permissions or
a Termux-compatible USB-serial bridge. Android microphone/speaker streaming
also needs an adapter; `termux-microphone-record` and media-player commands
are file/control APIs rather than the low-latency bidirectional PCM path used
by the Linux ALSA adapter. Those adapters must preserve stable host-owned IDs,
client selection, bounded queues, and explicit PTT safety.
