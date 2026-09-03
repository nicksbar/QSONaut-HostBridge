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

The protocol must not expose a raw serial or raw TCP transport to the client.
Driver-level raw protocol access, where Rigwright explicitly advertises it, is
available as a bounded request/response service on the authenticated session;
the host still owns the physical transport, lease, and driver instance.

For the first production media implementation, one WebSocket connection is
acceptable. Audio frames must remain small (normally 20–40 ms) so control
messages are not stuck behind large media writes. A separate media connection
can be added later without changing hardware ownership or control semantics.

## Host ownership rules

1. The host enumerates and opens radios and audio devices.
2. Clients see stable opaque IDs and labels, then supply their selected driver
   configuration; they never see `/dev` paths, COM names, ALSA names, or other
   host handles.
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

## Driver-owned scope services

Scope lifecycle belongs to the client. Selecting a radio only advertises the
driver's scope capability; it does not configure or start a scope stream.
Clients that receive a scope-capable radio may apply settings and control the
stream explicitly:

```json
{
  "type": "configure_scope",
  "request_id": "scope-config-1",
  "config": {
    "center_mode": true,
    "span_hz": 500000,
    "hold": false,
    "reference_level_tenths_db": 0,
    "sweep_speed": 1,
    "vbw_wide": false
  }
}
{ "type": "start_scope", "request_id": "scope-start-1" }
{ "type": "stop_scope", "request_id": "scope-stop-1" }
```

`start_scope` returns an acknowledgement and may be followed by an initial
`scope_frame`. Subsequent `scope_frame` messages contain complete driver
sweeps. The client owns retry and recovery policy; HostBridge only dispatches
these explicit operations and forwards frames. Model validation remains in
Rigwright.

## Driver service parity

HostBridge forwards the remaining generic Rigwright services without adding
client workflow:

```json
{ "type": "get_link_health", "request_id": "health-1" }
{ "type": "raw_protocol", "request_id": "raw-1", "frame": [254, 254, 0, 224, 253] }
```

The replies are `link_health` and `raw_protocol`. Link health preserves the
driver's optional counters and latency measurements. Raw protocol is only
accepted after a radio lease is held and is dispatched to Rigwright unchanged;
the client is responsible for knowing the selected driver's frame contract.

## Current implementation gap

The current runtime has versioned stream metadata, explicit media direction,
client-to-host media validation through `AudioSink`, selectable host playback
outputs through `AudioOutputProvider`, structured request/media errors, bounded
frame sizes, lag reporting, heartbeat pings, and PTT cleanup on session loss.
It also exposes client-owned scope lifecycle, tuner status, driver link health,
and driver-level raw protocol access.
The Linux executable supplies dynamic Rigwright and ALSA adapters. Actual RF
transmission remains a separate hardware/operator validation: media delivery
does not key PTT or claim that a modem/radio chain is configured.
