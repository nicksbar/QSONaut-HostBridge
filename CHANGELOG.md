# Changelog

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
