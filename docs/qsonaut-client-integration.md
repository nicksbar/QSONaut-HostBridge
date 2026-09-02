# QSONaut client integration guide

This is the client-facing guide for implementing HostBridge protocol v2 in
QSONaut. The goal is that a QSONaut developer can implement the connector from
this document and the protocol crate, without inspecting HostBridge runtime
internals.

## Connection and session lifecycle

Connect to the configured WebSocket endpoint. The current default deployment
uses plain `ws://`; TLS/private-tunnel support is a deployment requirement
before exposing the service outside a trusted LAN.

The client state machine is:

```text
Disconnected
  -> Connecting
  -> Send hello
  -> Receive host hello and capabilities
  -> Refresh catalog / show resources
  -> Acquire radio lease and/or select audio
  -> Operate
  -> Close or heartbeat timeout
  -> Disconnected
```

Every new connection is a new session. The client must not assume that a radio
lease, audio selection, frequency, mode, or PTT state survives reconnect.

## Authentication

The first text message must be:

```json
{
  "type": "hello",
  "protocol_version": 2,
  "client_name": "QSONaut",
  "access_key": "station-1",
  "password": "configured-secret"
}
```

The host replies with `hello`:

```json
{
  "type": "hello",
  "protocol_version": 2,
  "session_id": "uuid",
  "host_name": "Pi station",
  "capabilities": { }
}
```

Do not display or persist the password in QSONaut logs. Treat the session ID as
diagnostic metadata, not as a replacement credential.

## Capabilities and selection

`capabilities.radio_devices` is the authoritative host catalog snapshot. Each
radio entry contains:

- opaque stable `id` selected by the client;
- human-readable `label`;
- Rigwright `driver` and optional `model`;
- `transport` metadata;
- current `in_use` state.

The client sends:

```json
{ "type": "select_radio", "device_id": "host-advertised-id" }
```

The host opens the selected driver lazily after acquiring its exclusive lease.
A failed open is an error response; it is not permission for the client to
guess a device path or silently try another radio.

`capabilities.audio_sources` contains host-owned capture inputs. Each source
has an opaque `id`, label, kind, and exact supported formats. Select one with:

```json
{
  "type": "select_audio",
  "enabled": true,
  "source_id": "host-advertised-audio-id",
  "format": {
    "codec": "pcm_s16_le",
    "channels": 1,
    "sample_rate_hz": 48000
  }
}
```

Disable it explicitly when no longer needed:

```json
{
  "type": "select_audio",
  "enabled": false,
  "source_id": "host-advertised-audio-id",
  "format": {
    "codec": "pcm_s16_le",
    "channels": 1,
    "sample_rate_hz": 48000
  }
}
```

Audio capture selection does not select a radio, enable PTT, or imply a TX
path. `audio_playback` is a separate capability and must be true before the
client sends client-to-host media.

## Radio control

After selecting a radio, the client may send:

```json
{ "type": "set_frequency", "frequency_hz": 14074000 }
{ "type": "set_mode", "mode": "USB" }
{ "type": "set_ptt", "enabled": false }
{ "type": "get_state" }
```

The host replies with `ack`, `state`, or `error`. PTT must always be explicit;
audio stream activity never keys the radio. The client should send
`set_ptt: false` before disconnecting and must treat host cleanup as the final
fail-safe, not as a substitute for normal client behavior.

## Text responses and liveness

Successful commands use:

```json
{ "type": "ack", "request_id": null }
```

Errors are structured and should be surfaced with both fields retained:

```json
{
  "type": "error",
  "code": "request_failed",
  "message": "radio device is already in use"
}
```

The current protocol has an optional request ID field on acknowledgements but
does not yet attach request IDs to every client command. QSONaut should
correlate commands in order for now and keep the API ready for request IDs in a
future protocol revision.

The host periodically sends:

```json
{ "type": "ping", "nonce": 42 }
```

The client must reply:

```json
{ "type": "pong", "nonce": 42 }
```

The client may also send `ping` and expects a `pong`. On disconnect or missed
heartbeat, all leases are released and the host attempts to force PTT off.

## Binary media frames

WebSocket binary messages contain a 32-byte little-endian
`MediaFrameHeader`, followed immediately by the payload.

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | header version (`1`) |
| 1 | 4 | stream ID (`u32`) |
| 5 | 1 | direction (`0` host-to-client, `1` client-to-host) |
| 6 | 1 | codec (`0` PCM S16LE) |
| 7 | 8 | sequence (`u64`) |
| 15 | 8 | timestamp in samples (`u64`) |
| 23 | 4 | sample rate in Hz (`u32`) |
| 27 | 1 | channel count (`u8`) |
| 28 | 4 | payload length (`u32`) |

The payload length must equal the binary message length minus 32. For PCM S16LE,
the payload is interleaved signed little-endian samples. The current Pi emits
20 ms frames at 48 kHz:

- mono: 960 samples, 1,920 payload bytes;
- stereo: 960 samples per channel, 3,840 payload bytes.

The client must track sequence gaps, report/drop lost frames, and use a bounded
jitter buffer. It must not replay media from an old session or stream ID.

Host-to-client receive audio is currently the implemented media path. The
client-to-host frame validator and `AudioSink` boundary exist, but the current
Pi executable advertises `audio_playback: false`; QSONaut must not send
client-to-host media until a host advertises that capability.

## Reference Pi catalog

The reference Pi at the time of this guide reported:

- IC-7300, driver `icom_civ`, model `IC-7300`, USB serial;
- mcHF, driver `yaesu_legacy_cat`, model `FT-817ND`, USB serial;
- `hw:CARD=mchf,DEV=0`, PCM S16LE, stereo, 48 kHz;
- `hw:CARD=CODEC,DEV=0`, PCM S16LE, mono, 48 kHz.

QSONaut must render the received catalog dynamically. These IDs and labels are
examples from one host, not a compile-time device list.

## QSONaut implementation checklist

- [ ] Add a HostBridge endpoint configuration and credential storage path.
- [ ] Connect using WebSocket and send protocol-v2 `hello`.
- [ ] Render dynamic radio/audio catalogs from `HostHello.capabilities`.
- [ ] Select radio by advertised ID and handle lease errors.
- [ ] Select exact audio source and format by advertised ID.
- [ ] Decode 32-byte media headers and feed PCM into the QSONaut audio path.
- [ ] Track sequence/timestamp gaps and bound buffering.
- [ ] Handle host `ping` with `pong`.
- [ ] Force local PTT off on disconnect and reconnect.
- [ ] Treat reconnect as a new session and reacquire all resources.
- [ ] Keep client-to-host media disabled until `audio_playback` is advertised.
- [ ] Add integration fixtures for the reference catalog and binary frames.
