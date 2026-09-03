# Changelog

## Unreleased

- Keep scope lifecycle and configuration in clients; HostBridge only advertises
  the selected driver's scope capability and forwards scope frames.
- Add explicit client-owned scope configuration, start, and stop operations.
- Expose tuner status, driver link-health counters, and bounded raw driver
  protocol requests through the authenticated session.
- Pin Rigwright 0.1.21, including resilient IC-7300 unsolicited scope start.

## [0.1.0] - 2026-09-01

- Initial authenticated HostBridge protocol and runtime scaffold.
- Remote Rigwright radio selection and exclusive device reservations.
- Client-selectable host audio sources with binary PCM transport.
- Linux, Raspberry Pi ARM64, Windows, and macOS release targets.
- Selectable bidirectional ALSA PCM media with bounded host playback queues.
