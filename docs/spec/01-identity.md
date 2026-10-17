# 01. Identity

**Status:** Draft.

## 1. Intent

A person is identified by an AT Protocol DID. LP supports `did:plc` and `did:web`. The
DID document's verification methods and service endpoints are the anchor from which all
device trust is derived. See [ADR 0002](../adr/0002-atproto-did-identity.md).

## 2. Handles

A DNS handle (for example `alice.example.org`) maps to the DID via the standard atproto
handle resolution (DNS TXT record or `/.well-known/atproto-did`). Handles are for humans;
all protocol logic uses the DID. Open question: whether LP needs its own handle verification
step or can rely entirely on the PDS.

## 3. DID resolution

Devices resolve DIDs using the standard PLC directory or `did:web` HTTPS resolution, with a
local cache. Open question: cache lifetime and what a device does when resolution fails
while offline (likely: trust the last-known document and surface a warning).

## 4. Rotation keys and recovery

`did:plc` rotation keys allow the DID document to be updated or recovered. LP treats
rotation keys as the ultimate recovery root; chapter 07 describes how guardians and
Shamir shares protect them. Open question: for `did:web`, where the equivalent of a
rotation key lives.

## 5. Identity-to-device binding

The identity signs each device's public keys; chapter 02 defines the cross-signing
structure. This chapter only requires that the DID document (or a record in the user's
Space) publish the user's signing key used for that purpose.

## 6. Open questions

- Should LP require a PDS at all for the single-device bootstrap case?
- How are identity changes (handle change, PDS migration) propagated to devices?
