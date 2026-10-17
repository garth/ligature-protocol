# Glossary

Terms used across the Ligature Protocol specification and codebase.

**Abandonment**: A signed statement that an old master key is no longer the person's.
Gossiped after a rebuild.

**Binding**: See *user-device binding*. Also the role a signature carries when it marks a
key as my account on a device.

**Capability**: A typed function a device offers to peers, such as clipboard access or
media control. Identified by a reverse-DNS type name. Always live, never persisted.

**Capability descriptor**: One capability offered right now, with type, scope and
type-specific parameters. Sent in a manifest.

**CBOR**: Concise Binary Object Representation, RFC 8949. LP's wire encoding. Decoders
accept any valid CBOR; nothing requires a canonical form.

**Ceremony**: The proximity pairing step: an introducer verifies a key physically in front
of it and signs it with a role. One ceremony makes the newcomer known to all my devices.

**Device security**: How a device authenticates its local user, locks, and protects its
storage. Out of scope for the protocol, which defines authority and trust only.

**Device key**: A device's principal or binding public key in multibase form. It is the
device's identifier; there is no separate id. A short fingerprint shown in a terminal is
presentation only.

**Device principal**: The device itself: its own key, system-level daemon and owner set,
owner of the device-scoped capabilities.

**Device set**: All bindings signed by one person's self-signing key.

**Device-scoped capability**: A capability that belongs to the device rather than to any
account on it: display, speakers, sensors, physical input.

**Envelope**: The one container for a signed fact: type, payload bytes as the signer
produced them, signing key, and a signature over the prefix, type and payload. Facts are
verified over the bytes as received and never re-encoded.

**Gossip**: The exchange, after authentication at the start of every session, of signed
facts (revocations, bindings, principals, known persons, group membership, relays) that one
device holds and the other lacks.

**Grant**: A record authorising one device to use a capability of another, with scope and
expiry. Held by the device that issued it.

**Group**: A set of people, such as a household, with a group key, admins, a membership
document, group-owned devices and a policy. Membership alone grants nothing between
people; access to group-owned devices follows the group policy.

**Group key**: The signing key that is a group's identity, created by the founding admin
and held by every admin. It signs the membership document and is the owner of group-owned
devices.

**Group authority**: What a group device may do when nobody is signed in: its own
device-scoped capabilities for members under policy, other group devices' capabilities
under policy, and nothing of any member's user-scoped capabilities.

**Group device**: A device registered to the group rather than to a person (a lamp, a hub,
a family tablet, a television). Its owner set is the group key; every member may use its
device-scoped capabilities under the group policy after one pairing by an admin.

**Headscale**: An open-source implementation of the Tailscale coordination server. LP's
optional WireGuard integration generates configurations compatible with it.

**Helper**: A service that assists remote connectivity in one of three roles: discovery,
hole-punch coordination, or relay. Any helper may be operated by anyone, is listed in the
user's own records, and is never trusted with plaintext or identity.

**Hole punching**: Establishing a direct connection between two peers behind NAT by having
each send to the other's observed public address at the same time, with a coordinator
exchanging those addresses.

**Introducer**: A device of mine that holds the self-signing key and may therefore perform
the ceremony for new devices and known persons. Chosen per device at pairing time.

**Known person**: Another person whose master key I have verified in the ceremony and
signed with the `person` role.

**Least exposure**: The rule that data is encrypted only to the device that needs it; no
hub, relay or uninvolved device can decrypt it.

**Ligature**: Typographically, a glyph that joins two or more letters into one. The
protocol joins devices the same way.

**Local store**: The small persistent store each device keeps: its keys, device records
with role signatures, revocations, issued grants, group memberships and relays. The only
state LP has; there is no shared database.

**Manifest**: The list of capabilities a device currently offers, sent after
authentication and updated while the session lasts. Withdrawn entirely when it ends.

**Master key**: The signing key that is a person as far as the protocol is concerned.
Created on their first device; everything they trust chains to it. Written as a multibase
string with a multicodec prefix.

**Open sharing mode**: A proposed opt-in state in which a device accepts share offers from
anyone nearby for a few minutes. Off by default; an open question.

**Owner set**: The master keys, or the group key, allowed to administer a device
principal: set its policy and add other owners. The first person to pair a binding on an
unowned principal is its
first owner.

**Pairing page**: The basic web page a headless device serves on the LAN so the ceremony
can be completed from any trusted device without screen or camera hardware.

**pkarr**: Public-Key Addressable Resource Records: signed DNS-style records keyed by a
public key, published to the BitTorrent Mainline DHT. One of LP's discovery options.

**Person session**: A member signed in to a group device: a binding like any other, acting
with that person's full authority while the device reports it present. How the device
authenticates or locks it is device security, outside the protocol.

**Person fact**: The signed record of a person: master key, self-chosen display name, and
the self-signing and user-signing public keys with the master key's signatures over them.
Handed over in the ceremony and gossiped; what a verifier checks chains against and what a
prompt shows.

**Present**: Having an active, unlocked session on a device. The present person approves
cross-person requests for that device's device-scoped capabilities.

**Principal**: See *device principal*. Also the role a signature carries when it marks a
device as one I use.

**QUIC**: The UDP-based transport protocol of RFC 9000, with TLS 1.3 built in, stream
multiplexing and connection migration. LP's device-to-device transport and its end-to-end
encryption layer.

**Raw public key (RPK)**: The TLS mode of RFC 7250 in which a peer presents a bare public
key instead of a certificate. LP uses it to authenticate QUIC connections with device keys.

**Rebuild**: What replaces recovery: a new master key, re-pairing the devices one holds,
and re-verifying with household members.

**Relay**: A helper that forwards packets between two devices that cannot connect directly.
It sees only addresses and ciphertext. The default personal relay is the user's home hub.

**Role**: What a ceremony signature asserts about a key: `binding` (my account on a
device), `principal` (a device I use) or `person` (someone I know).

**Schema**: A JSON Schema 2020-12 document under `schemas/` defining one LP record or
message type. Messages are CBOR on the wire and validate against the schema after
decoding.

**Share**: The ad-hoc, AirDrop-like push of text, links or files to a chosen device,
accepted per item by the receiving person. A user-scoped capability.

**Registration**: Whether a device belongs to a person (*personal*) or to the group
(*group*), chosen by the introducer at pairing.

**Self-signing key**: The key, signed by the master key, that introducers hold to sign
bindings and principals.

**Signed fact**: A record in an envelope, signed by a device key or a person's key, that
any device can verify without a server. The unit of gossip; identified by the SHA-256 of
its payload bytes.

**Type name**: The reverse-DNS identifier of a schema or capability type, such as
`org.ligatureprotocol.capability.clipboard`. The domain is a placeholder until registered.

**User-device binding**: One person's account on one device: its own key, signed with the
`binding` role, member of their device set, owner of their user-scoped capabilities there.

**User-scoped capability**: A capability that belongs to a person's session on a device:
clipboard, notifications, personal files.

**User-signing key**: The key, signed by the master key, used to sign known persons and
group membership acceptances.

**UWB**: Ultra-wideband radio, usable for precise ranging to strengthen the ceremony where
hardware allows. Never required.
