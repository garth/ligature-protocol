# Glossary

Terms used across the Ligature Protocol specification and codebase.

**AT Protocol (atproto)**: The open protocol behind Bluesky. LP reuses its identity (DIDs,
handles), schema (Lexicons) and permissioned storage (Spaces) components.

**Capability**: A typed function a device offers to peers, such as clipboard access or
media control. Identified by an NSID.

**Capability descriptor**: A record describing one capability offered by one device, with
type-specific parameters.

**Cross-signing**: The Matrix-derived scheme in which a user's master key signs per-device
keys so that trust in the user implies trust in their devices.

**Device**: A single piece of hardware (or a software instance) holding its own keys and
belonging to a device set.

**Device set**: All devices trusted by one identity. Corresponds to one MLS group.

**DID**: Decentralized Identifier. LP supports `did:plc` and `did:web`.

**Grant**: A record authorising one device to use a capability of another, with scope and
expiry.

**Guardian**: A person or device holding a Shamir share of the user's recovery secret.

**Handle**: A DNS name (for example `alice.example.org`) that resolves to a DID.

**Headscale**: An open-source implementation of the Tailscale coordination server. LP's
remote overlay aims to be compatible with it.

**Lexicon**: atproto's schema language. LP schemas live under `org.ligatureprotocol.*`.

**Ligature**: Typographically, a glyph that joins two or more letters into one. The
protocol joins devices the same way.

**MLS**: Messaging Layer Security, RFC 9420. A group key agreement protocol providing
end-to-end encryption with efficient membership changes.

**NSID**: Namespaced Identifier, atproto's reverse-DNS naming for Lexicons, such as
`org.ligatureprotocol.device`.

**PDS**: Personal Data Server. The atproto server that hosts a user's repository and, in
LP, their Space.

**Rotation key**: In `did:plc`, a key authorised to update the DID document. LP treats it
as the recovery root.

**Space**: atproto's permissioned data store (alpha), used by LP for shared device state.

**Threshold share**: One piece of a secret split with Shamir's Secret Sharing; a threshold
number of shares reconstructs the secret.

**UWB**: Ultra-wideband radio, used for precise ranging in in-person verification.
