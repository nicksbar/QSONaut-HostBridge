# Changelog

## [0.1.6] - 2026-10-27

- Consume the published Rigwright 0.1.30 crate, inheriting the corrected
  IC-756PROII capability surface: RIT-offset and tuner-status reads are not
  advertised, and Split is write-only through the existing directional
  control capability projection.
- Correct the client integration guide's introduction and hello examples to
  control protocol v7. There is no wire change: the control protocol remains
  v7 and the binary media header remains v1.
- Keep radio leases, explicit PTT and disconnect cleanup, authentication,
  and the trusted-LAN/TLS security boundary unchanged.
- HostBridge hardware, audio, and physical PTT/RF behavior have not been
  validated for this update; upstream Rigwright hardware evidence does not
  constitute end-to-end HostBridge validation.

## 0.1.5 - Rigwright 0.1.29

- Consume the published Rigwright 0.1.29 crate to correct FT-991A filter-width
  CAT writes and readback through the existing HostBridge radio provider.
- Inherit model-owned `SH` framing: FT-991A uses `SH0nn;`, while FT-710,
  FTDX10, and FTDX101D/MP retain `SH00nn;`. No protocol version change is needed.
- Physical FT-991A confirmation of the reported post-preset timeout sequence
  remains pending; application bandwidth-preset selection is a separate issue.

## 0.1.4 - Rigwright 0.1.28

- Consume the published Rigwright 0.1.28 crate, including the IC-756 PRO
  family and Yaesu FT-1000/FT-1000D profile support.
- Keep the HostBridge protocol and runtime surface unchanged while exposing
  the new Rigwright model catalog through the existing selection and metadata
  paths.

## 0.1.3 - Rigwright 0.1.26 and WebSocket dependency refresh

- Consume the published Rigwright 0.1.26 crate, including the expanded
  model-aware driver validation, 90% coverage gates, and current CI/release
  fixes.
- Advance the control protocol to v7 and expose power, repeater settings,
  RIT/XIT, memory channels, DTMF, scope readback, IQ capability, and the
  corresponding model capability flags.
- Route selected radio operations through Rigwright's serialized
  `RadioSession`, including its bounded command/event handling and PTT safety
  watchdog.
- Advertise the selected Rigwright model's supported host-connection baud
  rates in `radio_capabilities.driver_metadata`, including the expanded Icom
  USB/native choices.
- Enumerate serial ports through Rigwright's serial-port stack on platforms
  without Linux `/dev/serial/by-id` links while keeping physical paths host
  local.

## [0.1.2] - 2026-09-04

- Add the HostBridge driver-metadata negotiation surface for Rigwright 0.1.22.
- Refresh selected-radio metadata after reconnect and reselection.
- Expose serialized driver/model metadata after radio selection, including
  scope geometry/options, control ranges and discrete values, and per-mode
  filter bandwidths. Values are projected from the instantiated Rigwright
  driver; HostBridge does not maintain model tables.
- Replay the last radio selection after reconnect so metadata is refreshed.

## [0.1.1] - 2026-09-04

- Keep scope lifecycle and configuration in clients; HostBridge only advertises
  the selected driver's scope capability and forwards scope frames.
- Add explicit client-owned scope configuration, start, and stop operations.
- Expose tuner status, driver link-health counters, and bounded raw driver
  protocol requests through the authenticated session.
- Use Rigwright 0.1.22, including model-aware native startup probing.

## [0.1.0] - 2026-09-01

- Initial authenticated HostBridge protocol and runtime scaffold.
- Remote Rigwright radio selection and exclusive device reservations.
- Client-selectable host audio sources with binary PCM transport.
- Linux, Raspberry Pi ARM64, Windows, and macOS release targets.
- Selectable bidirectional ALSA PCM media with bounded host playback queues.
