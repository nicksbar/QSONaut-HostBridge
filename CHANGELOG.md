# Changelog

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
