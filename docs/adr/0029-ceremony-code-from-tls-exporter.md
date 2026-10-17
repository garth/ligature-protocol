# 0029. The ceremony's short code is derived from the TLS exporter

**Status:** Accepted
**Date:** 2026-10-17

## Context

In the pairing ceremony (ADR 0017, chapter 02) an introducer connects to a newcomer whose
key it may or may not have scanned in full. When the full key is in the QR code and pinned,
a man in the middle fails on its own. The short code exists for the other cases: a code
typed from a headless device's pairing page, a television showing digits, a code read
aloud. It must bind to the specific TLS session so that an interposed attacker, who holds
two different sessions, cannot show the same code to both sides.

## Decision

1. Both devices derive the **short authentication string** from the TLS exporter of the
   pairing connection (RFC 8446 section 7.5, as RFC 9266 does for channel binding):

   ```text
   secret = TLS-Exporter(label   = "EXPORTER-ligatureprotocol-pairing-sas",
                         context = SHA-256(newcomer_key || introducer_key),
                         length  = 32)
   code   = first 20 bits of secret, shown as six decimal digits
            or as two words from a fixed word list for speaking aloud
   ```

2. The newcomer displays its code. The introducer either displays its own for the user to
   compare, or has the user type the newcomer's code and compares it itself. A mismatch
   aborts the ceremony and is shown to the user; one guess per attempt.
3. **Headless devices** show the code on their pairing page. The residual risk that anyone
   on the LAN can claim an unowned device during setup is accepted for milestone 1 and
   recorded in chapter 08; a claim window opened by a power cycle, or a printed code, are
   the options if that is later judged insufficient.
4. **Fallback.** If a TLS stack cannot expose the exporter, the code is derived from a hash
   of both raw public keys and a nonce exchanged inside the session, and the weaker binding
   is recorded as a gap. This is not expected with ADR 0028.

## Consequences

An interposed attacker has one chance in about a million per attempt, with a human
watching, which is the accepted bar for device pairing (Bluetooth numeric comparison uses
the same construction). The ceremony depends on the exporter being available, which task
B1 verifies. Chapter 02 section 4's question on code length and binding and chapter 03
section 6's question on binding the code to the session are closed.
