# 02. Device Binding

**Status:** Draft.

## 1. Intent

Every device holds its own long-term signing key and a separate key for MLS. Devices are
trusted because the user identity has cross-signed them, following the Matrix
cross-signing model. See [ADR 0003](../adr/0003-device-cross-signing.md).

## 2. Key hierarchy

The user identity holds a master signing key. It signs a self-signing key (which signs the
user's own devices) and a user-signing key, which signs the group memberships the user
accepts and the other identities they trust (chapter 10). Open question: whether the
user-signing key is needed in milestone 1, where there are no groups yet.

## 3. Adding a device

A new device is added by approval from an existing trusted device. The new device shows a
QR code or a short code; the existing device scans or enters it, verifies the new device's
key, and publishes a signature. Open question: the exact out-of-band channel for the
bootstrap when the devices are not co-located (likely: not supported in v1).

## 3a. Shared devices

A device with several user accounts has one **device principal** (its own key, owned by a
system-level daemon) and one **user-device binding** per account, each bound to that
account holder's identity by the flow in section 3. Device-scoped capabilities belong to
the principal; user-scoped ones to the binding. See chapter 10. Open question: whether the
device principal is paired to a group (chapter 10) or to its first owner.

## 4. Device records

Each trusted device has a record in the user's Space describing its keys, display name,
platform and current capabilities (see [`org.ligatureprotocol.device`](../../lexicons/org/ligatureprotocol/device.json)).

## 5. Revocation

Any trusted device can revoke another. Revocation removes the device record, removes the
device from the MLS group (chapter 03) and publishes a revocation so peers stop trusting
it. Open question: whether revocation should require more than one device when the device
set is large, to limit damage from a single compromised device.

## 6. Open questions

- How long may a device operate without re-verifying its signature chain against the Space?
- How are devices with no camera and no keyboard (hubs) paired?
