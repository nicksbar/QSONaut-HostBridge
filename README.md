# QSONaut HostBridge

HostBridge turns a small always-on computer, such as a Raspberry Pi, into an
authenticated radio endpoint for QSONaut. QSONaut connects to the host and
uses the selected Rigwright driver through a stable remote protocol; the
client does not need direct access to the host's serial or audio device paths.

## Current framework

- WebSocket control channel with JSON messages.
- Host-advertised physical radio catalog with client selection of USB/serial
  devices and Rigwright driver/model configuration.
- Exclusive radio reservations prevent concurrent control of one radio. This
  is session safety, not a permanent hardware lock.
- Binary PCM media frames with stream IDs, direction, timing, codec, and length.
- Dynamic host-side serial and ALSA discovery with stable client-visible IDs.
- Bidirectional 48 kHz S16LE media: selected ALSA inputs stream to QSONaut and
  selected ALSA outputs accept bounded client media queues.
- Heartbeats, bounded media frames, structured errors, and forced PTT-off on
  session loss.
- User-level Linux systemd service installation.

## Install and run on a Raspberry Pi

This is the native ARM64 bootstrap used on the reference Pi. HostBridge, Rust,
configuration, builds, and the service run as the station user. Only the OS
development prerequisite requires administrator access; it is needed by
Rigwright's serial-port support.

Install the one-time system prerequisite:

```sh
sudo apt-get update
sudo apt-get install -y pkg-config libudev-dev
```

Install Rust for the current user; this does not require sudo:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain stable
. "$HOME/.cargo/env"
```

Clone the two sibling repositories expected by the workspace:

```sh
git clone https://github.com/nicksbar/QSONaut-HostBridge "$HOME/QSONaut-HostBridge"
git clone --branch release/0.1.21 https://github.com/nicksbar/rigwright "$HOME/rigwright"
cd "$HOME/QSONaut-HostBridge"
```

Build and inspect the host catalog:

```sh
cargo build --locked --release --bin qsonaut-hostbridge
./target/release/qsonaut-hostbridge devices
```

The `devices` command shows radios, ALSA inputs, and ALSA outputs currently
visible to the station user. It is safe to run repeatedly and does not acquire
a radio lease.

Configure credentials and the listener. Use `127.0.0.1:8765` for local-only
clients; use `0.0.0.0:8765` for a trusted-LAN QSONaut client:

```sh
./target/release/qsonaut-hostbridge config set --bind 0.0.0.0:8765
./target/release/qsonaut-hostbridge config show
```

The interactive form avoids putting the password in shell history. Config is
stored at `$XDG_CONFIG_HOME/qsonaut-hostbridge/config.json` or
`~/.config/qsonaut-hostbridge/config.json`, with mode `0600`. `config show`
intentionally displays the password for easy station administration.

Install and control the background service without sudo:

```sh
./target/release/qsonaut-hostbridge service install
./target/release/qsonaut-hostbridge service status
./target/release/qsonaut-hostbridge service restart
./target/release/qsonaut-hostbridge service stop
./target/release/qsonaut-hostbridge service uninstall
./target/release/qsonaut-hostbridge config reset
```

`service install` writes a systemd user unit, enables it at login, and starts
it. Use `loginctl enable-linger "$USER"` if the host must keep running when
that user is not logged in; local policy may require administrator access for
that one command.

To update an existing checkout and rebuild:

```sh
cd "$HOME/QSONaut-HostBridge"
git pull --ff-only
cargo build --locked --release --bin qsonaut-hostbridge
```

For development, the foreground command also requires no sudo:

```sh
cargo run --locked -p qsonaut-hostbridge-app -- config set --key station-1 --password change-me --bind 127.0.0.1:8765
cargo run --locked -p qsonaut-hostbridge-app -- run
```

The automatic service installer currently targets Linux/systemd. The config
and foreground runtime are portable Rust code, leaving room for launchd or a
Windows-service adapter later.

## Android shell / Termux

HostBridge can also be built and run from an Android shell without root or
systemd. See [docs/android-termux.md](docs/android-termux.md), or run
`./scripts/install-termux.sh` after cloning. The protocol/runtime are portable,
but the current radio and ALSA adapters are Linux-specific, so Android needs
USB/audio adapters before it can enumerate and use local hardware.

## Raspberry Pi hardware adapter

The executable dynamically scans `/dev/serial/by-id` and ALSA at startup. Every
serial device is advertised once as a physical resource. The client selects
the Rigwright driver, optional model, baud rate, and radio address for that
resource; HostBridge validates the request and opens the host-private path.
HostBridge never infers a driver or model from a USB descriptor. A failed open
is reported to the client; the host does not silently try another driver or
permanently hide or lock the device. Hot-plug changes are discovered after
restarting HostBridge.

ALSA capture and playback cards are advertised dynamically by the station
user. Enumeration retains cards even when a temporary capability probe cannot
open one because another process is using it; selecting the card performs the
authoritative open and reports any real availability or format error.

The same dynamic catalog includes ALSA playback outputs. After the client
sends `select_audio_output`, HostBridge starts `aplay` for that host-owned
output and forwards client-to-host PCM through a bounded queue. Queue overflow
is reported as a media error; it never blocks radio control.

The client sees only stable IDs and labels. Raw serial paths remain host-local
and are opened lazily after an exclusive radio lease is acquired.

## Security boundary

The initial listener is plain WebSocket to keep the protocol testable. Do not
expose it beyond a trusted LAN until TLS or a private tunnel is configured.
The password is currently a runtime secret, not a persisted user database.
Production deployment should add TLS, rate limiting, credential rotation, and
an auditable host configuration before Internet exposure.
