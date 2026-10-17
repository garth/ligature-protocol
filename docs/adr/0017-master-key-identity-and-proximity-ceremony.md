# 0017. A per-person master key as the root of trust, with one proximity ceremony

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** [ADR 0002](0002-atproto-did-identity.md)

## Context

ADR 0002 made an AT Protocol DID the identity root. After ADR 0016 removed all shared
state, the DID was doing three jobs: naming the person above their keys so the keys could
rotate, anchoring recovery, and giving household members something stable to refer to.
Reviewing each: rotation matters only if there is state to preserve, and there is none;
recovery is now rebuilding (ADR 0018); and another person's master key is a perfectly
good stable reference once verified in person. What the DID cost was a mandatory server
dependency on first run (the PLC directory or a web host), a DNS handle, and an extra
adversary in the threat model. With AT Protocol no longer used anywhere, nothing specific
to it is kept.

Pairwise-only pairing (every device with every other, as KDE Connect does) was rejected:
a device verified by one of my devices must be available to all of them.

## Decision

1. **A person is a master signing key**, created on their first device. The protocol
   identifies people by this public key. Its textual form is a multibase string with a
   multicodec prefix, so the algorithm is self-describing.
2. **One proximity ceremony.** An existing device of mine that holds the self-signing key
   (an *introducer*) verifies a key physically in front of it and signs it with a **role**:
   - *binding*: this is my account on that device; member of my device set;
   - *principal*: this is a device I use; I may ask for its device-scoped capabilities;
   - *person*: this is another person's master key; their bindings are recognisably theirs.
   Every device of mine accepts anything carrying such a signature, so one ceremony makes
   the newcomer available to all of them.
3. **Key configuration follows the user's requirements.** The protocol requires only that
   signatures chain to the master key. The default configuration keeps a self-signing key
   on every introducer and the master key offline or derived; a simpler configuration uses
   one key for everything; a stricter one limits introducers to a few devices. Which
   devices are introducers is chosen at pairing time.
4. **The ceremony's baseline is a QR code plus a spoken short code.** BLE, NFC or UWB
   ranging may strengthen it where hardware allows but are never required.
5. **Headless devices never rely on hardware.** A device with no screen or camera serves
   a basic web page on the local network, reachable by a URL it prints, announces over
   mDNS or exposes on a well-known port; the page shows the code the ceremony needs. Any
   trusted device completes the pairing from that page.
6. **Publishing a key is optional.** A person may publish their master key at a DNS name,
   as a TXT record or at a well-known HTTPS path, to gain a human-readable name and a way
   to invite someone who is not in the room. Nothing in the protocol depends on it.

## Consequences

First run is "scan this code" with no account, no server and no internet. Transitive
trust holds with one ceremony per device. A leaked self-signing key has one remedy,
rebuild (ADR 0018), since there is no authority above it. Inviting a person who is not in
the room requires either the optional publication or an out-of-band exchange of the key.
Nothing AT Protocol-specific remains in the protocol; the schema language is decided
separately (ADR 0020).
