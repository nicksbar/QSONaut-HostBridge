# QSONaut HostBridge

HostBridge turns a small always-on computer (for example, a Raspberry Pi) into
an authenticated radio endpoint for QSONaut. QSONaut connects to the host and
uses the selected Rigwright driver through a stable remote protocol; the client
does not need direct access to the host's serial or audio device paths.

## Current framework

- WebSocket control channel with JSON messages.
- Host-advertised radio catalog with client selection of the USB/serial device
  and Rigwright driver/profile.
- Exclusive radio reservations: a radio selected by one client is reported as
  `in_use` and rejected for other sessions until the first session disconnects
  or selects another radio.
- Binary little-endian PCM audio frame seam, with codec negotiation in the protocol.
- Explicit radio capabilities and selectable host-side audio sources. The
  client receives each source's stable ID, label, kind, and supported formats,
  then selects the exact input to stream.
- One access key/password authorizer seam, with a static implementation for the
  first executable.
- A Raspberry Pi adapter that discovers the configured stable-by-id USB
  interfaces for an IC-7300 and mcHF, opens the selected Rigwright profile,
  and captures their ALSA USB audio inputs as 48 kHz PCM.

## Install and run

The executable has a guided, user-level Linux installation path. Build or
download the executable, then run:

```sh
qsonaut-hostbridge config set
qsonaut-hostbridge config show
qsonaut-hostbridge service install
```

`config set` asks for the access key and password, and accepts `--bind` and
`--name` for non-default listener settings. The configuration is stored at
`$XDG_CONFIG_HOME/qsonaut-hostbridge/config.json` (or
`~/.config/qsonaut-hostbridge/config.json`) with mode `0600`. This first
version intentionally keeps the password readable through `config show`, as
requested for easy station administration.

The service commands are deliberately simple:

```sh
qsonaut-hostbridge service status
qsonaut-hostbridge service restart
qsonaut-hostbridge service stop
qsonaut-hostbridge service uninstall
qsonaut-hostbridge config reset
```

`service install` writes a systemd user unit, enables it for the user's login,
and starts it. It does not require root. Use `loginctl enable-linger "$USER"`
if the host must keep running when that user is not logged in. The service
installer currently targets Linux/systemd; the config and foreground `run`
command are portable Rust code, leaving room for a launchd/Windows-service
adapter later.

For development, the foreground command is useful:

```sh
cargo run -p qsonaut-hostbridge-app -- config set --key station-1 --password change-me
cargo run -p qsonaut-hostbridge-app -- run
```

The executable currently includes a Raspberry Pi adapter for the observed
station hardware:

- `usb-Silicon_Labs_CP2102_USB_to_UART_Bridge_Controller_IC-7300_02015102-if00`
  opens the `IC-7300` Icom CI-V profile at 115200 baud.
- `usb-UHSDR_Community__based_on_STM_Drivers__USB_Interface_mchf_00000000002A-if00`
  opens the `FT-817ND`-compatible classic CAT profile at 4800 baud.
- `hw:CARD=CODEC,DEV=0` and `hw:CARD=mchf,DEV=0` are advertised as named ALSA
  capture sources and streamed as 48 kHz S16LE PCM.

The client sees only these stable IDs and labels. The raw serial paths remain
host-local and are opened lazily after an exclusive radio lease is acquired.

Radio selection follows the same pattern: a `RadioProvider` enumerates stable
host-side IDs and opens the selected Rigwright driver. The current adapter is
intentionally configured for this Pi's two radios; a later installer/config
surface should discover and persist additional station profiles.

This supports the normal profile workflow: connect a different USB radio to the
host, refresh the host catalog, select the new device/driver from QSONaut, and
reconnect later without allowing two sessions to control the same hardware.

## Security boundary

The initial listener is plain WebSocket to keep the protocol testable. Do not
expose it beyond a trusted LAN until TLS (or a private tunnel) is configured.
The password is currently a runtime secret, not a persisted user database.
Production deployment should add TLS, rate limiting, credential rotation, and
an auditable host configuration before Internet exposure.
