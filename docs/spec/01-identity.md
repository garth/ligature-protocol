# 01. Identity

**Status:** Draft.

## 1. Intent

A person is identified by a **master signing key**. It is created on their first device,
it signs the keys that sign their devices, and other people pin it after verifying it in
person. There is no account, no server and no name required. See
[ADR 0017](../adr/0017-master-key-identity-and-proximity-ceremony.md).

## 2. The master key

The master public key is the person as far as the protocol is concerned. Its textual form
is a multibase string with a multicodec prefix, so the algorithm is self-describing (an
Ed25519 key renders as `z6Mk…`). Signatures over device bindings, device principals and
known persons (chapter 02) must chain to it. Open question: whether to mandate Ed25519
alone or allow a second algorithm from the start.

## 3. Key configuration

The protocol requires only that signatures chain to the master key. How the private keys
are arranged is the user's choice, made to their own requirements:

- **Default.** A self-signing key, signed by the master key, is held by every device the
  user marks as an *introducer*; a user-signing key, likewise signed, is used to record
  known persons. The master private key is kept offline or derived from a passphrase.
- **Simplest.** One key serves as master, self-signing and user-signing key and lives on
  every introducer. Appropriate when rebuilding (chapter 07) is cheap, which it is.
- **Strictest.** Introducers are limited to one or two devices; everything else can be
  paired but cannot pair others.

Open question: whether the default should be the simplest configuration for first-run
usability, with the split keys offered as an upgrade.

## 4. Optional publication

A person may publish their master key at a DNS name, as a TXT record or at a well-known
HTTPS path. This adds a human-readable name (`alice.example.org`) and a way to invite
someone who is not in the room. Nothing else in this specification depends on it. Open
question: the exact record and path formats, and whether a published key should also list
the person's relays (chapter 04).

## 5. Referring to other people

Another person is known by their master key once it has been verified in the ceremony of
chapter 02 and signed with the *person* role. Their bindings are then recognisably theirs
on every one of my devices, which is what a grant request needs (chapter 10). Open
question: whether a known person's key should carry a display name signed by me, by them,
or both.

## 6. Open questions

- Should a master key carry an expiry to encourage periodic rebuilds, or is that hostile
  to users?
- Whether two people who have both published their keys can skip the in-person step when
  inviting each other remotely, and what that costs in the threat model.
