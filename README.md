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
- `NullRadio` wiring so the service can build and run before physical adapter
  configuration is added.

## Run the scaffold

```sh
QSONAUT_HOSTBRIDGE_KEY=station-1 \
QSONAUT_HOSTBRIDGE_PASSWORD='change-me' \
cargo run -p qsonaut-hostbridge-app
```

The current executable intentionally uses Rigwright's `NullRadio`. The next
adapter should construct the appropriate Rigwright profile/driver and an
`AudioSource`. That adapter should enumerate the Raspberry Pi's USB/radio
audio inputs as `AudioSourceInfo` records and subscribe to the selected source;
neither requires changing the wire protocol.

Radio selection follows the same pattern: a `RadioProvider` enumerates stable
host-side IDs and opens the selected Rigwright driver. The executable currently
publishes only `null-radio`; physical USB/serial enumeration is the next adapter
step.

This supports the normal profile workflow: connect a different USB radio to the
host, refresh the host catalog, select the new device/driver from QSONaut, and
reconnect later without allowing two sessions to control the same hardware.

## Security boundary

The initial listener is plain WebSocket to keep the protocol testable. Do not
expose it beyond a trusted LAN until TLS (or a private tunnel) is configured.
The password is currently a runtime secret, not a persisted user database.
Production deployment should add TLS, rate limiting, credential rotation, and
an auditable host configuration before Internet exposure.
