# 06. Local State and Gossip

**Status:** Draft.

## 1. Intent

Ligature Protocol has no shared database. Each device keeps a small local store of signed
facts, and devices exchange the facts the other lacks whenever they meet. Capabilities are
never stored; they are announced live (chapter 05). The only required off-device state is
the DID document (chapter 01). See
[ADR 0016](../adr/0016-no-required-shared-state.md).

## 2. What each device stores

- Its own device key(s) and, for the identity's first device, the cross-signing keys.
- The identity's public cross-signing keys, so any device's binding can be verified.
- Trusted device records it has learned: key, display name, platform, kind, with their
  cross-signatures (chapter 02, [`org.ligatureprotocol.device`](../../lexicons/org/ligatureprotocol/device.json)).
- Revocations it has learned.
- Grants it has issued (chapter 05) and, optionally, grants it has received.
- Group memberships it is party to (chapter 10).
- MLS state, where MLS is in use (chapter 03).

Nothing here is a cache of a remote truth; the local store *is* the truth as far as that
device knows. Open question: whether received grants should be stored or always re-sent by
the grantor on connect.

## 3. Signed facts

Every fact that travels between devices is signed and self-verifying:

| Fact | Signed by | Verified against |
| --- | --- | --- |
| Device binding | The identity's self-signing key | The identity's master key, from the DID document or a previously trusted copy |
| Revocation | Any trusted device key, or the self-signing key | The trust store |
| Group membership acceptance | The member's user-signing key | The member's master key |
| Group membership document | A group admin's device key | The group's admin list |
| Standing grant | The granting device's key | The trust store |

A device accepts a fact only if the signature verifies against a key it already trusts or
can trace to the DID document. Open question: whether facts carry a sequence number per
signer so that an older fact can never replace a newer one.

## 4. Gossip

When a session is established (chapter 04) the two devices exchange what the other lacks,
in this order: revocations, device bindings, group membership, then anything else. Both
sides apply what verifies and discard the rest. Gossip is small (a household has tens of
facts, not millions), so a full exchange of fact identifiers on every connect is
acceptable. Open question: whether to use a Merkle-style summary instead of a plain list
once groups grow.

## 5. Revocation and the offline window

A device that was offline when a peer was revoked trusts that peer until it next meets any
other device carrying the revocation. Mitigations, all of which implementations should
apply:

- Gossip revocations first, before any capability traffic.
- Where MLS is in use, a removed device cannot follow the group past the removal epoch.
- A device that has seen no trusted peer for a configurable period treats its trust store
  as stale and requires a fresh binding check before honouring sensitive capabilities.

Open question: the default staleness period, and whether a revocation should also be
published to the DID document for devices that come back online far from any peer.

## 6. Backup

An implementation may export a device's local store, encrypted to a key only the user
holds, to any storage they choose, including a PDS. No other device depends on that backup
and the protocol does not define it. Open question: whether a standard backup format is
worth specifying so implementations can restore each other's exports.

## 7. Open questions

- Should the guardian list (chapter 07) be the one fact that is also written to the DID
  document, since it must survive the loss of every device?
- How a device that has lost its local store but still holds its key re-learns the trust
  set: from any peer, by gossip, after the peer re-verifies it.
