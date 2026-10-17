# 09. Threat Model

**Status:** Draft.

## 1. Intent

This chapter records who LP defends against, what it protects, and what it explicitly does
not. It should be read alongside every other chapter; design choices elsewhere cite it.

## 2. Assets

The user's identity (DID control), the device set membership, the content of
device-to-device traffic, secrets at rest, and the user's presence and location
information.

## 3. Adversaries

- A passive network observer on the LAN or the internet.
- An active network attacker able to inject or modify traffic.
- A malicious or compromised PDS operator.
- A compromised device within the set.
- A malicious guardian, or a coalition of guardians below threshold.
- A physical attacker with brief access to an unlocked device.

## 4. Security goals

Confidentiality and integrity of peer traffic against network attackers and the PDS.
Authenticity of device membership. Recovery that cannot be completed by fewer than the
threshold of guardians, nor without the time delay. Open question: forward secrecy and
post-compromise security targets beyond what MLS provides by default.

## 5. Privacy goals

Local discovery advertisements should not reveal the user's identity to bystanders. The
PDS should learn as little as possible beyond the existence of records. Open question: what
metadata the PDS necessarily learns and whether that is acceptable.

## 6. Out of scope

A fully compromised operating system on a device, a coalition of guardians at or above
threshold acting together with an attacker, and denial of service by a network operator.

## 7. Open questions

- Should a compromised device be able to revoke all others? See chapter 02.
- How does the model change when household members join?
