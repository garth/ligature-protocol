---
name: security-audit
description: USE to audit the repository against the protocol's threat model and a checklist for a Rust peer-to-peer daemon and its Android app, as a full dated report or as a read-only pre-release gate.
---

# Security Audit

A review of the specification, the schemas, the Rust workspace and the Android app against
the threat model in `docs/spec/09-threat-model.md` and the checklist below. Two modes run
the same checks and differ only in what they produce.

| Mode                     | Invoke with                                               | Produces                                                                           |
| ------------------------ | --------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| **Full audit** (default) | `/security-audit`                                         | A dated report under `docs/security-audit/`, with a trend against the previous one |
| **Pre-release check**    | `/security-audit prerelease` (also `release`, `go-no-go`) | A GO / NO-GO verdict in the conversation, no file                                  |

The pre-release check must not create, modify or delete any file.

## Where findings go

The repository is public and `SECURITY.md` routes vulnerabilities through GitHub Security
Advisories, never issues. So: a Critical or High finding is fixed, or reported through an
advisory, **before** a report naming it is committed; the report records it as resolved or
as reported privately, with the advisory's reference and no exploit detail. A Medium or
lower finding may be recorded openly. The project is pre-alpha and unaudited, and the
report says so.

## Process

1. **Fan out with the Explore agent**, one agent per checklist group, in one message. Give
   each the previous report's findings for its group so it reports New, Resolved or
   Unchanged rather than rediscovering everything.
2. **Verify** every finding at the cited `file:line` before reporting it; for Critical or
   High, establish the path from attacker-controlled input (a peer on the LAN, a stranger
   that connects, a crafted fact, a crafted file name) to the sink. Confirm a claimed fix in
   the code, not the commit message. Report findings with confidence 7 or higher only.
3. **Produce the output** for the mode: the report file, or the verdict.
4. **Full audit only: point the repository at the new report.** `SECURITY.md` (the Audits
   section) and the README's documentation list each link the latest report by file name;
   move both links to the new file, in the same commit as the report, and check that
   `CONTRIBUTING.md` still links the directory. A report nobody can find from the front of
   the repository is not published.

## Checklist

### 1 Trust and authentication

The ceremony (QR offer, the code from the TLS exporter, ADR 0029), role signatures, the
stranger kept for pairing only, revocation gossiped first and honoured at the next session,
abandonment. Search `lp-identity`, `ligatured::ceremony`, `ligatured::run`
(`recognise`, "pairing only"), `gossip.rs`. Verify nothing is sent to an unrecognised peer
beyond the pairing offer, and that a revoked key cannot reconnect, re-pair or act through a
stale session.

### 2 Signatures and facts

Byte-exact verification of `"ligature/fact/1" || type || payload` before decoding, no
re-encoding, no canonical form (ADR 0035); the subordinate-key chain for roles; a fact's
version and quarantine handling in the trust store; the conformance vectors' invalid cases.
Search `lp-core::envelope`, `lp-crypto`, `lp-identity::facts`, `trust_store.rs`.

### 3 Transport and sessions

TLS 1.3 with raw public keys both ways, rustls default suites, no 0-RTT (ADR 0028); ALPN
`ligature/1`; the stream header; the 1 MiB message bound enforced on receive before
allocation; CBOR decoding depth and size; keep-alive and idle timeouts; the crossed-dial
tie-break and the no-replacement rule for a live connection (LP-117); what a peer can do
by opening many streams. Search `lp-transport::quic`, `stream.rs`, `manager.rs`.

### 4 Discovery

mDNS carries `v` and `id` only, `lp-<id>` as instance and host name, nothing identifying,
`id` a hint never a credential (ADR 0037); a spoofed record causes at worst a failed dial;
what the offer text leaks (addresses) and to whom. Search `lp-transport::mdns`,
`daemon.dialable_addrs`, `ceremony.rs`.

### 5 Capabilities and authority

The manifest filtered by audience; a request from a device that is not the person's rejected
before any effect (`same_person` checks in every sync); the implicit grant only for the
same person; nothing persisted from a manifest. Search `ligatured::capabilities`,
`grants.rs`, every `*_sync.rs` `apply`.

### 6 Files on disk

Received files land under the download folder with a base name only, never a path; the
name clash rule; part files renamed after the hash check; the clipboard cache under the
runtime or cache directory and removed; symlinks and `..` in names; the sizes announced
against the bytes received; MediaStore pending rows on Android. Search `files.rs`,
`files_sync.rs`, `clipboard_sync.rs` (cache), the Android `files.rs` and `Transfers.kt`.

### 7 Local interfaces

The control socket: directory 0700, socket 0600, peer uid checked on accept, the runtime
directory's defaults; the key file 0600; the Wayland helper's stdio frames; the Android
`ContentProvider` (`ClipboardProvider`) exported surface and what it serves to which caller;
exported activities, receivers and services in `AndroidManifest.xml`. Search
`ligatured::control`, `paths.rs`, `clipboard_linux.rs`, the manifest.

### 8 Resource limits

Bounds on text size, icon bytes, announced bytes kept (64 MiB), memory fetches (16 MiB),
`maxStreams`, the availability window; what an authenticated peer of the same person can
exhaust. Report only where a bound is missing or unenforced; denial of service by a stranger
is out of scope below.

### 9 Secrets and material

No private key, seed or token in the repository beyond `vectors/keys.json` (test material,
labelled); the Android keystore only through the environment and the workflow's secrets;
logs never print key material or message bodies at `info`. Search `git ls-files`, the
workflows, `tracing::info!` calls.

### 10 Dependencies and build

`cargo deny check` (advisories, licences, bans, sources) clean, with every ignore carrying a
reason; `Cargo.lock` committed; the Android library's `cargo ndk` targets; GitHub Actions
pinned to a major that runs on the current Node; `unsafe` only in the two allowed crates
and each block justified.

### 11 Threat model coverage

Each goal in chapter 09 sections 4 and 5 (security goals, helpers, discovery records, relay
denial of service, privacy goals, no remote trust) has either an implementation that meets
it, a note that the layer is unimplemented, or a finding.

### 12 Injection and parsing

Every SQLite statement in `trust_store.rs` uses bound parameters, never a formatted string;
every decoder from a peer (CBOR in `lp-core::wire`, JSON in the control channel and the
schemas' tests, the pairing offer text) bounds what it reads and rejects what it does not
know how to handle, ignoring unknown fields only; a file name, a device name or a person's
display name is treated as data wherever it is shown or written. Search `execute`, `query`,
`format!` near SQL, `ciborium::from_reader`, `serde_json::from`.

### 13 Content passed through

The clipboard carries HTML, RTF and images and the notifications family carries bodies,
titles and icons between a person's devices. LP hands them to the platform untouched and
never renders, interprets or executes any of them itself; the Android app shows text with
Compose, never HTML. Verify no path parses or displays content as markup, that an icon is
checked against its announced hash and bound before it is shown, and that a content URI the
provider serves cannot be made to read outside what was announced.

### 14 Security logging

A rejected fact, a frame over the bound, a stranger's connection, a refused request and a
failed hash check are logged at a level an operator sees (`info` or `warn`) with the peer's
key and the reason, and never with the content itself. Search the `Err` branches of every
`apply` and the `warn!` and `info!` calls beside them.

### 15 OWASP Top 10 cross-check

Walk the ten categories against the protocol: A01 access control is authority and audience
filtering (item 5); A02 cryptographic failures are items 2 and 3; A03 injection is item 12;
A04 insecure design is the threat model's open questions (item 11); A05 misconfiguration is
the units, the fixed port, the firewall note and the Android manifest (item 7); A06
vulnerable components is item 10; A07 authentication failures are item 1; A08 integrity is
item 2 and the release artefacts (signed APK, tarballs with no signature yet); A09 logging is
item 14; A10 SSRF has no equivalent (the daemon makes no requests to URLs a peer supplies)
and is recorded as not applicable.

## Exclusions

- Untracked and uncommitted files (`git ls-files` decides).
- Denial of service by an unauthenticated stranger (the threat model accepts it on a LAN and
  no relay exists yet); report only what an authenticated peer can exhaust.
- Theoretical issues with no concrete path; test files; documentation.
- The unimplemented layers (remote discovery, hole punching, relays, groups): note them as
  unimplemented, do not audit stubs.

## Confidence

High (8 to 10): a clear path from input to effect. Medium (5 to 7): a pattern that needs
specific conditions. Low (1 to 4): theoretical. Report 7 and above.

## Full audit output

Write `docs/security-audit/ligature-security-audit-YYYY-MM-DD.md`:

```markdown
# Ligature Protocol security audit

**Date:** … **Branch:** … **Commit:** …

## Summary

## Trend since the previous audit

## Findings by checklist item

### 1 Trust and authentication

**Status:** PASS | FAIL | PARTIAL **Risk:** Critical | High | Medium | Low | Info
**Change:** New | Resolved | Unchanged

**Evidence:** `file:line`, what the code does
**Recommendation:** what to change

[2 to 15 likewise]

## Other observations

## Critical findings

| Item | Status | Risk | Finding |

## Deferred or accepted risk

| Item | Risk | Deferred since | Reason | What would unblock it |

## Recommendations in priority order
```

Keep the 100-column rule and markdownlint happy. Do not mention Claude or Claude Code.
Then update the latest-report links in `SECURITY.md` and the README (step 4 of the
process) before committing.

## Pre-release check

Answers: is it safe to tag what is on this branch?

1. **Baseline**: the newest report under `docs/security-audit/`; if none, say so and treat
   every finding as new.
2. **Scope**: the commits since the baseline report was added
   (`BASE=$(git log --diff-filter=A --format=%H -- docs/security-audit/<file> | tail -1)`;
   `git log --oneline "$BASE"..HEAD`), and a clean working tree (`git status --porcelain`);
   uncommitted work is not releasable.
3. **Run the checklist**, all fifteen items, weighted to the changed surface, without skipping the rest.
4. **Classify** each finding against the baseline: New, Regressed, Unchanged–Deferred,
   Unchanged–Open, Resolved.
5. **Dependencies**: `cargo deny check advisories` in `impl/`, compared with the baseline;
   an advisory is a blocker when the vulnerable code is reachable from a peer or a file.
6. **Verdict**, first matching rule wins:
   - **NO-GO**: a New or Regressed Critical or High; any open Critical; a reachable New or
     Regressed High advisory; a deferred finding materially worse; a dirty tree touching
     security-relevant code.
   - **GO WITH CONDITIONS**: an Unchanged–Open High never formally deferred, or a New or
     Regressed Medium.
   - **GO**: nothing New or Regressed above Low, every remaining High Unchanged–Deferred
     with a recorded reason.

Output in the conversation, verdict first, then why, blocking issues with remediation,
non-blocking, accepted risk carried, resolved since the baseline, dependency status, and one
line per checklist item. Report honestly; fix nothing during the check; say what could not
be checked and how it affects confidence. Do not mention Claude or Claude Code.
