# Milestone 1: two Linux devices pair and share the clipboard

**Status:** Plan. Reflects the design as of ADR 0024. Supersedes the plan sketched before
ADRs 0011 to 0024, which assumed a PDS, DIDs, MLS and a Space; none of those remain.

## 1. Goal

One person. Two Linux machines on the same LAN (or two daemons on one machine with
separate data directories). Pair them with the proximity ceremony, then copy text on one
and paste it on the other, in both directions, with no server, no account and no internet.
Revoke one and show it can no longer connect. Restart both and show the pairing survived.

## 2. What the current design makes unnecessary

Compared with the first sketch, this milestone no longer needs DID resolution, a PDS or
Space, OpenMLS, guardians, or any storage integration. It is: keys, one ceremony, QUIC with
raw public keys, a handful of gossiped facts, a manifest, one clipboard stream.

## 3. Non-goals for this milestone

Groups and group devices (chapter 10), relays, hole punching and the DHT (chapter 04
section 5), key publication (chapter 01 section 4), the `share` family, non-text clipboard
content, BLE, headless pairing pages, Android or iOS, the system-level principal daemon as
a separate process (section 6, phase F), and the optional WireGuard integration.

## 4. Decisions to pin before coding

Each is a short ADR; none should take long.

1. **Signature algorithm.** Ed25519 only for now. Multibase/multicodec rendering of public
   keys as specified in chapter 01.
2. **Wire encoding of messages.** Decided: CBOR from the start
   ([ADR 0025](../adr/0025-cbor-wire-encoding.md)). Byte fields are CBOR byte strings,
   timestamps and keys stay text, signed facts use core deterministic encoding. `lpctl`
   may print messages as diagnostic notation for debugging.
3. **QUIC stack.** Must support TLS 1.3 raw public keys (RFC 7250) on both sides. In Rust,
   quinn over rustls is the candidate; rustls has raw-public-key support. Whatever is
   chosen for the reference implementation under ADR 0014 must clear this bar, which is a
   real constraint on TypeScript, where no mainstream QUIC stack offers RPK today.
4. **Binding the ceremony code to the session.** The short code shown on both sides is
   derived from the TLS exporter of the pairing connection (RFC 9266 style channel binding),
   so a relayed impostor cannot complete the ceremony. This resolves the open question in
   chapter 02 section 4 for the proof of concept.
5. **Implementation language for this milestone.** The recommendation is to build it in
   the existing Rust workspace regardless of the outcome of ADR 0014, because of item 3,
   and to treat a TypeScript reference as milestone 2 once conformance vectors exist.

## 5. Exit criteria

- Two freshly initialised daemons pair in one ceremony started from `lpctl pair`.
- After pairing, each daemon's `lpctl devices` lists the other as a binding of the same
  person with a verified role signature.
- Text copied on either machine appears in the other's clipboard within a second on a LAN.
- `lpctl revoke` on either side stops the other from authenticating on its next connection
  attempt, and the revocation survives restarts.
- Both daemons restart with their trust store intact and reconnect without a new ceremony.
- Every message exchanged is CBOR that, decoded, validates against a schema under
  `schemas/`, and the fixtures used in tests are committed as the first conformance
  vectors.

## 6. Work breakdown

Each task is intended to be one ticket and one commit. Phases may overlap where their
dependencies allow.

### Phase A: keys and local store

- A1. Ed25519 key generation and multibase rendering; `PersonKey`, principal key, binding
  key types in `lp-core` and `lp-crypto`.
- A2. Simplest key configuration (chapter 01 section 3): one key serves as master,
  self-signing and user-signing key. The split configuration is a later task.
- A3. Role signature format and verification (`lp-identity::cross_signing`): sign the
  deterministic CBOR encoding of a key-and-role fact with `binding` or `principal`, verify
  the chain to a master key. Unit tests become the first conformance vectors.
- A4. Local store on disk under `$XDG_DATA_HOME/ligature/`: own keys, device records with
  signatures, revocations. CBOR files, the same encoding as the wire. Load, save,
  round-trip tests.
- A5. Revocation fact: create, verify, apply to the store (`revoked` flag retained).

### Phase B: transport

- B1. QUIC endpoint with raw-public-key TLS (`lp-crypto::session`,
  `lp-transport::quic`): listen, dial, present the device key, read the peer's.
- B2. Recognition check on connect: the peer key is a binding of mine (chain verifies
  against my master key) or the connection proceeds no further than the pairing offer.
- B3. mDNS/DNS-SD advertise and browse (`_ligature._udp`), with a random per-boot
  identifier in the TXT record as the placeholder for the per-epoch identifier of chapter
  04 section 4.1.
- B4. Connection manager: one connection per peer, reconnect on loss, stream open per
  capability type.
- B5. Integration test: two endpoints on loopback discover each other and open an
  authenticated connection.

### Phase C: the ceremony

- C1. Newcomer side (`lp-identity::pairing::Newcomer`): generate keys, show fingerprint and
  QR in the terminal, accept an incoming pairing connection, derive the short code from the
  TLS exporter, display it.
- C2. Introducer side (`Introducer`): scan or type the fingerprint, connect, derive the same
  code, ask the user to confirm it matches, sign the newcomer's binding (and principal) with
  the chosen roles, send signatures, the self-signing key (simplest configuration) and
  current gossip.
- C3. Both sides store the result; the newcomer is now an introducer too (configurable
  later).
- C4. `lpctl pair` (both roles) and `lpctl devices`.
- C5. Negative tests: wrong code, tampered fingerprint, connection replaced mid-ceremony.

### Phase D: gossip and manifests

- D1. Gossip exchange after authentication (`lp-identity::trust_store`): list fact
  identifiers, send what the peer lacks, revocations first; apply what verifies.
- D2. `lpctl revoke` and the propagation test: revoke on A, B learns on next connect to A.
- D3. Manifest after gossip (`lp-core::Manifest`): send current capabilities, update on
  change, withdraw on disconnect. `lpctl caps` shows the live view.
- D4. Message schemas for gossip and manifests under `schemas/`; tests decode CBOR and
  validate the result against the schema.

### Phase E: clipboard

- E1. `org.ligatureprotocol.capability.clipboard` schema: text/plain only, a `set`
  operation, a size cap.
- E2. Wayland clipboard watcher and setter via `wlr-data-control` (Hyprland is the first
  target); detect changes, suppress echoes by content hash.
- E3. Clipboard stream handler: on local change send to every connected binding of mine;
  on receive, set locally. Last-writer-wins by timestamp.
- E4. Implicit same-person grant recorded with basis `implicit` so the grant path exists
  even though no prompt is shown.

### Phase F: daemon plumbing

- F1. `ligatured` as a per-account systemd user service holding both the binding and, for
  this milestone, the principal key (a personal device's principal and binding share a
  process; the separate system-level daemon is deferred).
- F2. D-Bus interface for `lpctl` (zbus): status, pair, devices, revoke, caps.
- F3. Persistence across restart: keys, store, and reconnection without a new ceremony.

### Phase G: verification and write-up

- G1. End-to-end script: two daemons with separate `XDG_*` directories on one machine;
  pair, copy both ways, revoke, restart. Runs in CI on Linux.
- G2. Commit the test fixtures as the first conformance vectors under `vectors/`.
- G3. Update chapters 02, 03, 04, 05 and 06 with what was actually built, still marked
  draft; record any deviation as an ADR.

## 7. Risks

- The QUIC stack's raw-public-key support may be incomplete on one side (client or server
  authentication). Check in B1 before anything else is built on it.
- The Wayland clipboard path differs by compositor; `wlr-data-control` covers Hyprland and
  other wlroots-style compositors but not GNOME. Out of scope here, but the watcher should
  be behind a trait.
- Channel binding of the ceremony code depends on exporter support in the TLS library;
  if absent, fall back to comparing key fingerprints and record the gap.

## 8. After this milestone

Milestone 2 candidates, in no order: the split key configuration and the introducer
choice at pairing; the `share` family between two people (requires the `person` role and
the second master key); the headless pairing page; a group with one group device; relays
and the DHT for the away-from-home case; the TypeScript reference against the vectors
from G2, per ADR 0014.
