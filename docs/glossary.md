# Glossary

Terms used across the Ligature Protocol specification and codebase.

**AT Protocol (atproto)**: The open protocol behind Bluesky. LP reuses its identity (DIDs,
handles) and schema (Lexicons) components. LP does not use a PDS or Spaces.

**Capability**: A typed function a device offers to peers, such as clipboard access or
media control. Identified by an NSID.

**Capability descriptor**: A record describing one capability offered by one device, with
type-specific parameters.

**Cross-signing**: The Matrix-derived scheme in which a user's master key signs per-device
keys so that trust in the user implies trust in their devices.

**Device**: A single piece of hardware (or a software instance) holding its own keys and
belonging to a device set.

**Device principal**: The device itself on a shared device: its own key and system-level
daemon, owner of the device-scoped capabilities.

**Device set**: All devices (or user-device bindings) trusted by one identity. Corresponds
to one MLS group.

**Device-scoped capability**: A capability that belongs to the device rather than to any
account on it: display, speakers, sensors, physical input.

**DID**: Decentralized Identifier. LP supports `did:plc` and `did:web`.

**Gossip**: The exchange, at the start of every session, of signed facts (revocations,
device bindings, group membership) that one device holds and the other lacks.

**Grant**: A record authorising one device to use a capability of another, with scope and
expiry.

**Group**: A set of identities, such as a household, with admins, a membership record,
shared devices and a policy. Membership alone grants nothing; access is by grant.

**Guardian**: A person or device holding a Shamir share of the user's recovery secret.

**Handle**: A DNS name (for example `alice.example.org`) that resolves to a DID.

**Headscale**: An open-source implementation of the Tailscale coordination server. LP's
optional WireGuard integration generates configurations compatible with it.

**Helper**: A service that assists remote connectivity in one of three roles: discovery,
hole-punch coordination, or relay. Any helper may be operated by anyone, is listed in the
user's own records, and is never trusted with plaintext or identity.

**Hole punching**: Establishing a direct connection between two peers behind NAT by having
each send to the other's observed public address at the same time, with a coordinator
exchanging those addresses.

**Lexicon**: atproto's schema language. LP schemas live under `org.ligatureprotocol.*`.

**Local store**: The small persistent store each device keeps: its keys, trusted device
records, revocations, issued grants, group memberships and MLS state. The only state LP
has; there is no shared database.

**Ligature**: Typographically, a glyph that joins two or more letters into one. The
protocol joins devices the same way.

**Manifest**: The list of capabilities a device currently offers, sent when a session
starts and updated while it lasts. Withdrawn entirely when the session ends.

**MLS**: Messaging Layer Security, RFC 9420. A group key agreement protocol providing
end-to-end encryption with efficient membership changes.

**NSID**: Namespaced Identifier, atproto's reverse-DNS naming for Lexicons, such as
`org.ligatureprotocol.device`.

**pkarr**: Public-Key Addressable Resource Records: signed DNS-style records keyed by a
public key, published to the BitTorrent Mainline DHT. One of LP's discovery options.

**Present**: Having an active, unlocked session on a device. The present person approves
cross-identity requests for that device's device-scoped capabilities.

**PDS**: Personal Data Server, the atproto server that hosts a user's repository. Not used
by LP; a user needs a DID, not a PDS.

**QUIC**: The UDP-based transport protocol of RFC 9000, with TLS 1.3 built in, stream
multiplexing and connection migration. LP's device-to-device transport.

**Raw public key (RPK)**: The TLS mode of RFC 7250 in which a peer presents a bare public
key instead of a certificate. LP uses it to authenticate QUIC connections with device keys.

**Relay**: A helper that forwards traffic between two devices that cannot connect directly.
It sees only addresses and ciphertext. The default personal relay is the user's home hub.

**Rotation key**: In `did:plc`, a key authorised to update the DID document. LP treats it
as the recovery root.

**Signed fact**: A record signed by a device key or a cross-signing key that any device
can verify without a server. The unit of gossip.

**Space**: atproto's permissioned data store (alpha). Considered for shared state and
dropped (ADR 0016).

**Threshold share**: One piece of a secret split with Shamir's Secret Sharing; a threshold
number of shares reconstructs the secret.

**User-device binding**: One person's account on one device: its own key, cross-signed by
that person's identity, member of their device set, owner of their user-scoped
capabilities there.

**User-scoped capability**: A capability that belongs to a person's session on a device:
clipboard, notifications, personal files.

**UWB**: Ultra-wideband radio, used for precise ranging in in-person verification.
