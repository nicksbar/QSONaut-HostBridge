# HostBridge protocol contract

This document defines the HostBridge rules that QSONaut clients must follow.
It is intentionally transport-oriented: hardware paths, serial ports, and
audio device handles never become client-visible protocol data.

## Transport decision

HostBridge uses an authenticated WebSocket connection over TCP (TLS is required
when the connection leaves a trusted private network).

The connection carries:

- JSON text messages for authentication, capabilities, leases, radio control,
  state, errors, and heartbeats.
- Binary messages for audio/media frames.

The protocol must not add a raw serial or raw TCP hardware-control mode. That
would duplicate framing and security work while providing little benefit for
the expected 48 kHz radio audio rate. A future transport may replace the
WebSocket implementation behind the same session and message contracts only
if measured WAN latency or loss justifies it.

For the first production media implementation, one WebSocket connection is
acceptable. Audio frames must remain small (normally 20–40 ms) so control
messages are not stuck behind large media writes. A separate media connection
can be added later without changing hardware ownership or control semantics.

## Host ownership rules

1. The host enumerates and opens radios and audio devices.
2. Clients see stable opaque IDs, labels, capabilities, and driver metadata;
   they never see `/dev` paths, COM names, ALSA names, or other host handles.
3. A client must authenticate before receiving the catalog.
4. A radio lease is explicit, exclusive, and released on disconnect or lease
   expiry. Selecting a different radio releases the prior lease first.
5. Audio selection is explicit and may be shareable according to the host
   adapter. Selecting audio never implicitly selects or keys a radio.
6. The host is authoritative for availability, state, and errors. The client
   must not infer availability from stale catalog data.

## Session rules

1. The first message is `hello`; protocol version and credentials are required.
2. The host returns a session ID and a complete capability snapshot.
3. Every request receives an acknowledgement or a structured error. Errors
   include a stable machine-readable code and human-readable detail.
4. Heartbeats detect dead clients. A dead session releases all radio leases and
   forces PTT off.
5. Reconnection creates a new session. A client must re-authenticate, refresh
   the catalog, and reacquire resources; leases are never silently resumed.
6. The host must bound per-client queues and frame sizes. A slow audio consumer
   may lose audio frames, but must not stall radio control or exhaust memory.

## Media rules

PCM S16LE at 48 kHz remains the canonical modem format. It is approximately
768 kbps for mono audio and is preferred because modem tones and weak-signal
waveforms should not pass through a lossy speech codec.

Every binary media frame needs, at minimum:

- protocol/media header version;
- stream ID and direction (`host_to_client` or `client_to_host`);
- codec and format;
- monotonically increasing sequence number;
- sample timestamp or sample position;
- sample rate and channel count;
- payload length, followed by the payload.

The receiver must detect sequence gaps, maintain a bounded jitter buffer, and
report loss. It must not replay stale audio after a reconnect or stream reset.

The protocol may negotiate Opus later for an operator-monitoring stream or
bandwidth-constrained WAN link. Opus must not silently replace the canonical
modem stream. Any compressed mode requires explicit negotiation and a decoder
available on both sides.

## Radio and TX safety

1. No radio command is valid until its radio lease is held by the session.
2. PTT is explicit and must never be implied by audio selection or stream
   activity.
3. Disconnect, lease expiry, stream failure, and service shutdown force PTT
   off where the driver supports it.
4. A client must receive current state after selection and may request refresh;
   cached state is not authoritative.
5. Bidirectional media is a separate capability. Host-to-client receive audio
   does not imply client-to-host transmit audio.

## Current implementation gap

The current runtime has versioned stream metadata, explicit media direction,
client-to-host media validation through `AudioSink`, selectable host playback
outputs through `AudioOutputProvider`, structured request/media errors, bounded
frame sizes, lag reporting, heartbeat pings, and PTT cleanup on session loss.
The Linux executable supplies dynamic Rigwright and ALSA adapters. Actual RF
transmission remains a separate hardware/operator validation: media delivery
does not key PTT or claim that a modem/radio chain is configured.
