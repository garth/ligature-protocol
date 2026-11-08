# Ligature Protocol security audit

**Date:** 2026-11-08 **Branch:** main **Commit:** 723b108 audited, with the High finding fixed
at 69b9a54 (LP-185) before this report **Scope:** the specification, the schemas, the Rust
workspace and the Android app, against the threat model in chapter 09 and the fifteen-item
checklist of the `security-audit` skill. Second audit; the baseline is the
[audit of 2026-10-23](ligature-security-audit-2026-10-23.md), 134 commits earlier.

The project is alpha. Since the previous audit it gained milestone 7 (one keyboard and mouse
across the desktops, the phone as a touchpad, the phone driven through its accessibility
service), milestone 8 (a screen watched and driven from another device, the phone shared
with nobody at it once its permission is granted over `adb`), ADRs 0038 to 0046, signed
releases with `lpctl upgrade` (ADR 0042), the Mac's permissions handling, and the fixes of the
previous audit's three High findings (LP-120, LP-121, LP-122) and of several Mediums (LP-127,
LP-128). The one High finding of this audit was fixed the same day under LP-185 (ADR 0047)
before this report was committed, as `SECURITY.md` requires; it is described here in full
since the fix is in the tree.

## Summary

The core still holds: TLS 1.3 with raw public keys both ways, Ed25519 only, no resumption or
0-RTT, the frame bound checked before allocation, CBOR from a bounded slice, facts verified
byte for byte before decoding, streams held until the peer is admitted, SQL through bound
parameters, received files under a safe base name, the control socket private to the user,
no private material in the repository, no content in the logs. The three High findings of
the previous audit are fixed in the code, and the fixes hold under the new families: input
and screen refuse anything but the person's own bindings, a revocation closes a live drive
and a live share, and everything held is lifted however a drive ends.

The one High finding is the successor of the previous audit's first: the revocation rule of
ADR 0039 counted a revocation from any person whose role fact covered the subject, and a role
fact is one person's signature over any key. Any paired device could invent a person, sign
that person's role over a key of the set and that person's revocation of it, and gossip the
three facts; every device then treated the subject as revoked, and a lost device could revoke
the owner's introducers first so that its own revocation never reached the others. ADR 0047
narrows the rule to persons the device knows and keeps the owner's own keys out of any other
person's reach.

Below it, the new Medium findings cluster where the new code trusts a device of the same
person too far: the screen viewer takes the sharer's `started` and frames without bounds; a
Linux share goes on when its only indicator fails to post, and a viewer can close that
indicator; a peer that stops reading stalls the local release and the local stop; a fact's
identity ignores its signer, so a pre-planted copy can make the owner's own revocation inert;
and the release signing key sits in a repository-wide secret with a workflow-wide write
token. Seven Medium findings from the previous audit are unchanged.

| Risk | New | Unchanged open | Resolved since 2026-10-23 |
| --- | --- | --- | --- |
| Critical | 0 | 0 | |
| High | 1, fixed the same day (LP-185) | 0 | 3 (LP-120, LP-121, LP-122) |
| Medium | 5 | 7, and 2 deferred | 4, and 10-A in part |
| Low | 21 | 28 | 3 |
| Info | 8 | 7 | |

## Trend since the previous audit

- **Resolved:** 1-A, 1-B, 1-C (the Highs); 5-A grants no longer leave the device; 7-A the share
  sheet confirms before sending; 7-B only another app's `content:` URIs are opened; 7-C data
  extraction rules exclude the keys; 8-B notifications on macOS are posted in-process; 2-C a
  quarantined fact heals when a valid copy arrives; 6-D the clipboard cache no longer falls
  back to `/tmp`; 10-A for upgrades through `lpctl`, which verify a signed checksum file;
  `cargo-ndk` is pinned (part of 10-B).
- **Unchanged:** every other finding, with 3-F widened by LP-158 and 7-F's reach widened by the
  new control requests.
- **New:** one High (1-H, fixed), five Medium (2-F, 5-D, 5-E, 8-F, 9-C), twenty-one Low and
  eight Info, nearly all in the input and screen families and the release pipeline.
- **Regressed:** none.

## Findings by checklist item

Evidence cites paths relative to the repository root at commit 723b108. Confidence is 1 to
10; only 7 and above are reported. Letters continue the previous report's.

### 1 Trust and authentication

**Status:** PARTIAL (FAIL at the audit; the High fixed the same day) **Risk:** Medium remaining
**Change:** 1-A, 1-B, 1-C Resolved; 1-H New and Resolved; 1-I, 1-J New

**Passes.** The ceremony derives its code on each side from its own connection, with the two
TLS keys as the exporter context (`impl/crates/lp-identity/src/pairing.rs:202-207`,
`impl/crates/ligatured/src/ceremony.rs:28-31, 88, 146`); the introducer refuses a peer whose
key is not the offer's (`ceremony.rs:140-142, 368-377`) and grants only after both confirm
(`pairing.rs:677-699`); the newcomer accepts a grant only when the person fact verifies, the
roles chain to it and cover exactly its keys, and the seed matches (`pairing.rs:460-508`).
Streams are held until admission (`impl/crates/ligatured/src/daemon.rs:175`,
`impl/crates/lp-transport/src/manager.rs:362-393`, at most 8 queued, reset with code 2), only
a trusted peer is admitted (`impl/crates/ligatured/src/run.rs:253-273`), strangers are dropped
when a ceremony ends (`daemon.rs:98-105, 528-534`), and the gossip handler checks recognition
itself (`impl/crates/ligatured/src/gossip.rs:89-94`). `verify_revocation` is called
(`impl/crates/lp-identity/src/trust.rs:98`). `enforce_trust` runs after every exchange, every
local revocation and every ceremony, drops a `Revoked` peer and detaches every family,
ending drives and shares both ways (`daemon.rs:885-899, 664-681`,
`impl/crates/ligatured/src/input.rs:634-645`, `impl/crates/ligatured/src/screen.rs:482-487`).
Recognition is recomputed from the store on every call (`trust.rs:145-200`).

**1-A, 1-B, 1-C.** Resolved, confidence 9 each, as the passes show.

**1-H. Any paired device could revoke the rest of the set by inventing a person.** High,
confidence 9. Fixed under LP-185 (ADR 0047). `is_revoked` counted a revocation when it
verified against the person fact of the person it named and a verified binding or principal
role signed by that person covered the subject (`trust.rs:85-115` before the fix), looking the
person up by master key alone. A role fact proves nothing about whose a key is, and gossip
stores any self-consistent fact from a trusted peer (`impl/crates/lp-identity/src/gossip.rs:93-106`,
`impl/crates/lp-identity/src/trust_store.rs:573-580`). So a paired device, an introducer or
not, could generate a person, sign its role over any key of the set and its revocation of
it, and gossip the three; every device then saw the subject as `Revoked`, and a revocation of
the owner's master key locked every device out of its own person (`trust.rs:126-128,
154-156`). A lost device could revoke the introducers first, after which the others refused
their gossip and the owner's revocation of it never arrived: the defence of chapter 09
section 4.4, defeated. The LP-120 test covered a stranger's revocation and another person's
without a role, not a role the attacker signed. Now a revocation by anyone but the owner
counts only from a person the owner knows (a `person` role from the owner, not revoked) and
never over a key the owner's own roles cover; an abandonment of a person's own master key
stands. Tests: a trusted device's forged person against a device and against the master key,
a known person's role over the owner's device, a stranger revoking their own device.

**1-D. Whoever holds the offer text can race to become the introducer.** Medium, confidence
9. Unchanged: the newcomer takes the first `pairing` stream (`ceremony.rs:316-320`),
`on_hello` accepts any key that knows the one-time code (`pairing.rs:412-425`), no introducer
name or key is shown, and `lpctl pair --yes` skips the comparison
(`impl/crates/lpctl/src/main.rs:1369`). Recommendation as before.

**1-E. Every new binding is an introducer and gets the self-signing seed.** Medium, deferred
by design, confidence 9. Unchanged (`ceremony.rs:436`, `pairing.rs:694-696`). A revoked
introducer can still sign roles and revocations as the owner.

**1-F. Revocation is per key; `introduce` accepts a revoked offer key.** Low, confidence 8.
Unchanged (`impl/crates/ligatured/src/control.rs:567-583`, `ceremony.rs:341-397`): the
ceremony completes, the grant with the seed goes out by default, and only `enforce_trust`
drops the peer afterwards. Recommendation: refuse revoked offer keys before dialling; revoke a
device's two keys together.

**1-G. Short-code bias; word-list form, offer staleness and abandonment unimplemented.**
Info. Unchanged (`pairing.rs:220-221`; no daemon path makes an `Abandonment`).

**1-I. A peer that stops being trusted without being revoked keeps admission while a
ceremony runs.** Low, confidence 8. New. `enforce_trust` drops an untrusted but unrevoked peer
only when no ceremony accepts strangers (`daemon.rs:890`); a binding of a person the owner
has stopped knowing recognises as `Stranger`, stays `Admitted` and keeps every stream until
the ceremony ends. Latent while no `person` role exists. Recommendation: demote or drop every
admitted peer that is no longer trusted, whatever the ceremony state.

**1-J. A revocation learnt by gossip is not pushed on to connected peers.** Low, confidence
8. New. After an exchange only `enforce_trust` runs (`gossip.rs:95-99`, `daemon.rs:646-649`);
`gossip_all` runs for local revocations and introductions only, so a device whose session is
with a relaying peer keeps its session with the revoked device until its next reconnect, short
of ADR 0039 decision 5 in a mesh that is not fully connected. Recommendation: push
revocations when an exchange stores a new one.

### 2 Signatures and facts

**Status:** PARTIAL **Risk:** Medium **Change:** 2-C Resolved; 2-E, 2-F New

**Passes.** The signature is checked over `"ligature/fact/1" || type || payload` as received,
before decoding (`impl/crates/lp-identity/src/facts.rs:65-68, 133-134`); the type name is
checked before decoding (`:76-86`); payloads are stored and forwarded byte-exact
(`trust_store.rs:577-598`); subkeys verify under a domain-separated message
(`impl/crates/lp-identity/src/cross_signing.rs:42-65`); stored rows are re-verified at open
against identifier, signature and index columns (`trust_store.rs:607-637`); the vectors gained
an outsider-signed revocation and a person fact with an unknown field.

**2-B. A revoked signer's earlier facts stay valid.** Low, by design. Unchanged.

**2-C. A quarantined fact never healed.** Low. Resolved, confidence 9: `insert` upserts a
quarantined row when a copy arrives (`trust_store.rs:577-600`), `apply` checks `usable`, and
`ids()` leaves quarantined rows out so `want` asks again (`:404-421`).

**2-D. `verify` not `verify_strict`; no length separator; vector gaps.** Info, partly
resolved. The gaps now are a revocation by another person with and without a self-claimed
role, a small-order key, and the same payload under two signers (2-F).

**2-E. Whoever holds the self-signing key chooses the person fact's version.** Low,
confidence 8. New. `verify_person` needs only the self-signing key with the master's existing
endorsements (`facts.rs:132-151`) and the newest version wins (`trust.rs:57-70`), so any
introducer or a leaked seed can publish `version: u64::MAX`, after which a master-key rotation
never takes effect. This is chapter 06 section 3's open question. Recommendation: have the
master key sign the version, or bound version jumps.

**2-F. A fact's identity ignores its signer, and the first copy stored wins.** Medium,
confidence 7. New. The identifier is the SHA-256 of the payload (`facts.rs:90-92`); `insert`
keeps an existing usable row whatever its signer (`trust_store.rs:577-600`); `apply` and `want`
never ask for a second copy. A trusted device can store in advance the exact revocation
payload the owner will later sign over it (one `revoked_at` per second of the expected
window, each copy signed by itself); the owner's revocation then inserts nothing and the copy
that spreads fails `verify_revocation`, so it is inert everywhere. Needs a trusted device going
rogue before it is revoked, which is the lost-device case. Recommendation (LP-189, with an
ADR): identify a fact by payload and signer, or keep every verifying copy and prefer the one
whose chain verifies.

### 3 Transport and sessions

**Status:** PARTIAL **Risk:** Medium **Change:** 3-A Resolved; 3-H, 3-I, 3-J New

**Passes.** TLS 1.3 only, raw public keys both ways, client authentication mandatory, no
tickets, resumption or early data (`impl/crates/lp-crypto/src/session.rs:126-131, 237-239,
273-294`); exactly one Ed25519 key, no intermediates (`:121-139, 151-157`); ALPN `ligature/1`
(`:30, 280, 292`); the header's type checked and a malformed one reset with code 4
(`impl/crates/lp-transport/src/stream.rs:96-107`, `manager.rs:354-360`); 1 MiB checked before
allocation (`impl/crates/lp-core/src/frame.rs:75-81`); ciborium's recursion limit of 256 over a
bounded body; keep-alive 15 s, idle 60 s (`impl/crates/lp-transport/src/quic.rs:34-51`); the
crossed-dial tie-break symmetric (`manager.rs:132-153, 296-331`); a stopping daemon closes
its connections (`run.rs:204-207`). Input and screen refuse anything but `MyBinding`
(`input.rs:903-907`, `screen.rs:595-599`); messages from the wrong side are refused or ignored
(`input.rs:750-773, 1082-1084`, `screen.rs:562-564, 805-807`).

**3-A.** Resolved, see item 1.

**3-B. A flood of facts breaks gossip for good.** Medium, confidence 9. Unchanged: no cap on
facts per exchange or signer (`impl/crates/lp-identity/src/gossip.rs:93-106`); `have` lists
every id in one frame, which fails past about 30,000 facts (`impl/crates/ligatured/src/gossip.rs:37`).
Recommendation under LP-189.

**3-C. Known persons' devices get the full gossip.** Low, latent. Unchanged.

**3-D. Header reads serial and untimed, allocating the announced length.** Low, confidence 9.
Unchanged (`manager.rs:352-361`, `stream.rs:69-70`): about 100 MiB per authenticated
connection.

**3-E. quinn limits are defaults.** Low, confidence 9. Unchanged (`quic.rs:44-51`): 100
unidirectional streams never accepted, about 125 MB buffered per connection.

**3-F. Dialling an offer can replace a live connection.** Low, confidence 8. Unchanged and
widened: under LP-158 `connect` replaces any live connection older than 3 s
(`manager.rs:132-153`), and a wrong key at an offer address drops that peer without
`on_disconnected` (`ceremony.rs:374`), leaving stale state until the reconnect loop.

**3-H. Concurrent `start` requests each launch a capture pipeline.** Low, confidence 8. New.
The busy check precedes `capture.start` and the recheck follows it (`screen.rs:650-706`), so
up to 100 streams carrying `start` at once get up to 100 pipelines, and possibly prompts,
before all but one are stopped. Recommendation: reserve the display before starting.

**3-I. The driven side has no cap on an incoming batch and injects under a mutex.** Low,
confidence 7. New. `MAX_BATCH` applies on send only (`input.rs:51, 555`); `apply` walks the
whole batch under the `driven` lock with a blocking pipe write per event on Linux
(`input.rs:996-1065`, `impl/crates/ligatured/src/input_linux.rs:247-282`); the schema's
`maxItems: 256` and `text` `maxLength: 4096` are not enforced, and onward events skip the
flush, so a relayed batch over 256 drops events, releases included (`input.rs:886-889, 555`).
Recommendation (LP-188): refuse over-long batches and text; flush onward at `MAX_BATCH`.

**3-J. The relay can loop around three or more devices.** Low, confidence 7. New. The guard
refuses only the device driving this one (`input.rs:457, 519, 925, 969, 983`); A driving B by
a viewer, B driving M across an edge and M driving A by a viewer circulate every event until
a drive ends, with the `onward` channel unbounded (`input.rs:217, 258`). The release chord
still works. Recommendation (LP-188): refuse a drive of a device that is driving this one
through the chain, or carry an origin.

### 4 Discovery

**Status:** PARTIAL **Risk:** Medium (privacy, deferred) **Change:** 4-F New

**Passes.** The record carries `v` and a random per-process `id` only, instance and host
`lp-<id>` (`impl/crates/lp-transport/src/mdns.rs:33-93`); `id` is used for filtering and
back-off only, recognition runs on the handshake key (`run.rs:313, 410-421`).

**4-A.** Medium, deferred by design. Unchanged (`run.rs:54-69`, `session.rs:276-279`).
**4-B.** Low. Unchanged: the first IPv4 address of any record is dialled with no scope check
(`run.rs:336`); the first in string order wins (`mdns.rs:187`). **4-C.** Low. Unchanged: the
fixed port against ADR 0037. **4-D.** Info. Unchanged. **4-E.** Low. Unchanged: the offer
carries every non-loopback IPv4 address, is a command-line argument, and is copied on the
phone without a sensitive flag (`pairing.rs:73-82`, `daemon.rs:585-600`,
`impl/android/app/src/main/kotlin/org/ligatureprotocol/android/MainActivity.kt:140-150`).

**4-F. A spoofed record can replace a live session and steer it through a relay.** Low,
confidence 7. New. A record with a fresh `id` whose address relays to a trusted peer passes
the `keys` and `connected_at` guards (`run.rs:339-355, 383-402`), the handshake succeeds, and
under LP-158 the live connection is replaced on both ends and the relay stored as the peer's
last address (`manager.rs:132-153, 318-331`, `run.rs:256`). Confidentiality holds; the attacker
can cut the session at will, which is more than ADR 0037's "at worst a failed dial".
Recommendation: skip a discovered dial when the key it reaches has a live connection.

### 5 Capabilities and authority

**Status:** PARTIAL **Risk:** Medium **Change:** 5-A Resolved; 5-D, 5-E, 5-F New

**Passes.** Every family refuses anything but the person's own bindings before any effect:
input (`input.rs:696-698, 903-907`), screen (`screen.rs:499-500, 595-599`), clipboard
(`impl/crates/ligatured/src/clipboard_sync.rs:225, 555, 735`), files
(`impl/crates/ligatured/src/files_sync.rs:727, 1193, 1247, 1374`), notifications
(`impl/crates/ligatured/src/notification_sync.rs:143, 276`); the implicit grant is a record,
not an authority (`impl/crates/ligatured/src/grants.rs:100-140`); nothing from a manifest is
persisted (`impl/crates/ligatured/src/capabilities.rs:128-136`); one drive at a time each
side, never of the device driving this one; an absolute entry is refused where not accepted
(`input.rs:917-924`); a share is bound to its stream and peer (`screen.rs:684-711`); the
picture is fitted to the display and the rate clamped (`screen.rs:654-661`). Same-person
driving and watching need no local prompt, as chapter 10 and chapter 09 section 4.4 intend.

**5-A.** Resolved, confidence 9: `all_facts` drops `LOCAL_ONLY_TYPES` (`daemon.rs:608-616`),
and grants are refused from peers and never offered (`impl/crates/lp-identity/src/gossip.rs:69,
94-99`, `pairing.rs:282-285`).

**5-B. The manifest is not filtered by audience.** Low, latent. Unchanged
(`capabilities.rs:141-166`); it now carries input and screen too.

**5-C. A peer can dismiss notifications it never received.** Low, confidence 8. Unchanged
(`notification_sync.rs:327-331`); now reaches the sharing notification (5-D).

**5-D. A Linux screen share can run with no indicator.** Medium, confidence 8. New. Chapter
09 section 4.4 says a share never runs unseen; on Linux the daemon's notification is the only
indicator (`impl/crates/ligatured/src/screen_linux.rs:20-24`). If it cannot be posted the
share goes on with a warning (`screen_linux.rs:893-897`); only `ActionInvoked` is watched, so a
dismissal removes it (`:987-1027`); and a viewer can close it by sending a `dismiss` naming
the sharer as origin with the bus id, a small integer (`notification_sync.rs:327-329`,
`impl/crates/ligatured/src/notifications_linux.rs:197-203`). The portal's restore token and
the screencopy path repeat no consent. Recommendation (LP-187): refuse the share when the
notification cannot be posted, treat its closing as a stop or re-post it, and let
`dismiss_own` act only on ids sent to that peer.

**5-E. A stalled peer stream defeats the local release and the local stop.** Medium,
confidence 8. New. `run_drive` awaits `stream.send` inside its `select!` (`input.rs:733-743`); a
driven device that stops reading fills the drive's channel, `send` blocks before the release
chord is seen (`input.rs:604-609`), and `release_local` sends `Release` before it ends the drive
and gives the capture back (`input.rs:618-630`), so the keyboard and mouse stay captured until
the peer disconnects, against section 4.4's "the controller releases its capture at once".
`run_share` awaits `stream.send` per chunk (`screen.rs:770-786`), so `lpctl screen stop` and the
notification's Stop are not acted on while a viewer withholds credit. Recommendation (LP-188):
give the capture back before sending `Release`; send with a timeout and reset on lapse.

**5-F. Keys relayed onward are not lifted when the controller's drive ends.** Low, confidence
7. New. Relayed events skip the held-key tracking (`input.rs:1009-1013`) and `end_driven` lifts
only the local injector (`:669-685`), so a modifier A held stays down on C while B's drive of
C goes on, against chapter 05 section 3g. Recommendation under LP-188.

### 6 Files on disk

**Status:** PARTIAL **Risk:** Medium (6-A) **Change:** 6-D Resolved; 6-J New

**Passes.** `safe_name` keeps the base name, drops control characters, caps at 255 bytes
(`impl/crates/ligatured/src/files.rs:414-440`); the part file is claimed with `create_new`
and renamed after the hash and size checks (`files.rs:189-193`, `files_sync.rs:1443, 1459,
1632-1635`); the cache is under the runtime directory and emptied at start and per generation
(`clipboard_sync.rs:146, 676-685`); Android's pending rows are deleted on abort; the upgrade
tarball is verified before it is written to a fresh `tempdir` and unpacked with `tar`
(`impl/crates/lpctl/src/upgrade.rs:305-349`).

**6-A. A clipboard fetch trusts the fetch-time offer, and the memory landing has no cap.**
Medium, confidence 9. Unchanged (`clipboard_sync.rs:781-791`, `files_sync.rs:1113-1124, 1443`,
`files.rs:385-389`). Recommendation as before.

**6-B, 6-C, 6-E, 6-F, 6-G, 6-H.** Low. Unchanged, at the lines the previous report gave,
`files.rs` being byte-identical. **6-D.** Low. Resolved, confidence 9
(`impl/crates/ligatured/src/clipboard.rs:753`, `impl/crates/lp-core/src/paths.rs`). **6-I.**
Info, by design.

**6-J. `lpctl screen start --out` truncates its target and writes without limit.** Info,
confidence 7. New (`impl/crates/lpctl/src/main.rs:1000, 1015-1017`); the path is the user's
own. Recommendation: `create_new`, or a warning when the file exists.

Noted for defence in depth: `lpctl upgrade` relies on `tar`'s defaults against `..`, absolute
members and symlinks in a tarball that has already verified against the release key.

### 7 Local interfaces

**Status:** PARTIAL **Risk:** Low **Change:** 7-A, 7-B, 7-C Resolved; 7-B2, 7-G, 7-H New

**Passes.** Socket 0600 in a 0700 directory, the peer uid checked on every accept, frames
capped at 1 MiB (`control.rs:614-653`); a drive ends when the control client that started it
disconnects (`control.rs:749-755`); every helper is this executable spawned with fixed
arguments over stdio, with frames bounded (`input_linux.rs:45, 121`, `screen_linux.rs:61,
751-779`, `impl/crates/ligatured/src/clipboard_linux.rs:44`); `lpctl permissions reset` runs in
the client against its own bundle id (`impl/crates/lpctl/src/main.rs:373-389`); the Mac's tap
only listens unless a drive runs and logs codes only for dropped keys
(`impl/crates/ligatured/src/input_macos.rs:305, 472`); on Android the provider is not exported
and read-only, every pending intent explicit and immutable, the accessibility service
declares `canPerformGestures` and window-state events only, without `canRetrieveWindowContent`
(`impl/android/app/src/main/res/xml/drive_service.xml:5-11`), and its gestures and typing are
reached only through the node's callback; the media projection result is used once.

**7-A.** Resolved, confidence 9: the share sheet shows the text and needs a tap on Send
(`.../ShareActivity.kt:36-57`). **7-B.** Resolved, confidence 9: `isShareable` requires a
`content:` URI from another authority (`.../Transfers.kt:160-164, 198-201`). **7-C.** Resolved,
confidence 9: `dataExtractionRules` exclude `ligature/` from backup and transfer.

**7-B2. The own-provider check compares the raw authority.** Info, confidence 7. New. A user
id prefix (`0@`) defeats the string comparison (`Transfers.kt:201`); Android strips it when
resolving, so the app opens its own provider, which sends a peer's clipboard item on to a
device the user picks. Recommendation: compare the authority without a user id, or resolve
the provider and refuse this package.

**7-D. On macOS the daemon's own uid is the binary's owner.** Low, confidence 9. Unchanged
(`control.rs:658-681`). **7-E. Hardening gaps.** Low, confidence 8. Unchanged: the runtime
directory's mode set through a symlink with no owner check (`control.rs:630-636`), the key
file's temporary without `create_new` or `O_NOFOLLOW` and its mode unchecked on load
(`impl/crates/lp-identity/src/key_file.rs:55-95`), the clipboard helper's unbounded, untimed
read of another client's offer (`clipboard_linux.rs:636-662`), `install.sh`'s unescaped `sed`
(`packaging/install.sh:134`), and a systemd unit without hardening directives.

**7-F. Same-user control requests.** Info, accepted by ADR 0024, reach wider. Unchanged in
kind: to the earlier list, `input.drive`, `input.send`, `input.release`, the layout,
`screen.start`, `screen.stop` and `permissions.request` (`control.rs:791-807, 998-1037,
1162-1208`) let any same-user process drive and watch every device of the person's and make
macOS show Ligature's permission prompts. Recommendation: revisit ADR 0024 for `pair`,
`input.drive` and `screen.start`.

**7-G. Ligature's macOS privacy grants are usable by other same-user code.** Low, confidence
7. New. The bundle is signed ad hoc without the hardened runtime
(`packaging/macos-bundle.sh:62-66`), so `DYLD_INSERT_LIBRARIES` is honoured, and the agent's
plist is user-writable with a `PATH` that puts Homebrew first, from which `ffmpeg` is spawned
(`impl/crates/ligatured/src/screen/encode.rs:334`): a same-user process can run code inside a
process holding Accessibility, Input Monitoring and Screen Recording. Low because 7-F already
grants a same-user process the equivalent. Recommendation: `--options runtime`; resolve
`ffmpeg` to a checked absolute path.

**7-H. The phone can be watched and driven with nobody at it.** Info, confidence 8. New, by
design (LP-167): the `PROJECT_MEDIA` op is settable only over `adb` or by the system
(`.../ScreenShare.kt:56-68, 84-94, 166`), and the indicators are the foreground notification
and the cast icon. Recommendation: a persistent stop control while driven.

### 8 Resource limits

**Status:** FAIL **Risk:** Medium **Change:** 8-B Resolved; 8-F New; 8-G to 8-L New

| Bound | Enforced on receive | Evidence |
| --- | --- | --- |
| Clipboard text size | Yes | `clipboard_sync.rs` |
| 16 MiB memory fetch | Bypassable | 6-A |
| Icon bytes and hash | Yes | `notification_sync.rs:283-289` |
| Icon cache per session | None | 8-D |
| Input events per batch (256) | Sender only | `input.rs:51, 555`; 3-I |
| Text length (4096) | Linux only, by the helper frame | `input_linux.rs:45, 108`; 3-I |
| Picture size and rate | Yes, on the sharer | `screen.rs:654-661` |
| Chunk size (256 KiB) | Sender only | `impl/crates/lp-core/src/screen.rs:22, 493`; 8-F |
| Frame reassembly on the viewer | None | 8-F |
| Frames queued to the viewer | None | 8-F |
| Frames queued on the sharer | None while the stream stalls | 8-G |
| Keyframe requests | None | 8-H |
| Concurrent `start`s per peer | quinn's 100 streams | 3-H |
| Stall restarts | 3 per 60 s | `screen_linux.rs:73-113` |
| `onward` relay channel | None | 8-J |

**8-A.** Medium, see 6-A. **8-B.** Medium. Resolved, confidence 9: notifications are posted
in-process through the notification centre (`impl/crates/lp-macos/src/notifications.rs:190-225`).
**8-C, 8-D, 8-E.** Low. Unchanged; the input and screen advertise loops now wait behind 8-C's
lock each second (`input.rs:323-339`, `screen.rs:295-314`).

**8-F. The viewer takes the sharer's `started` and frames without bounds.** Medium,
confidence 9. New. `Started` is accepted with its width, height, rate, format and display
unchecked against the `start` sent (`screen.rs:415-446`); `run_watch` appends every chunk of a
`seq` to `pending` until one with `last` arrives, with no size cap and no check against
`CHUNK_BYTES`, and whole frames go into an unbounded channel (`screen.rs:535-553, 434`), so a
sharer that never sets `last`, or outruns the viewer, grows the viewer's memory without limit.
`lpview` allocates `w * h * 4` twice from `started` (`impl/crates/lpview/src/main.rs:397-401,
705`): an overflow or allocation abort at absurd sizes, a spin at zero; the Android app's
`aspectRatio` fails at zero or a negative value. Recommendation (LP-186): reject a `Started`
outside the request, cap a frame, bound the channel, `checked_mul`.

**8-G. The sharer's frame queue is unbounded while the viewer stalls.** Low, confidence 8.
New (`screen_linux.rs:820`, `impl/crates/ligatured/src/screen_macos.rs:167`,
`impl/crates/ligature-android/src/screen.rs:152`; drained only between sends,
`screen.rs:754-765`). Under LP-188.

**8-H. Keyframe requests are not rate-limited, and each restarts ffmpeg.** Low, confidence
8. New (`screen.rs:801-803`, `encode.rs:659-721`). Under LP-188.

**8-I.** The schema's input bounds unenforced: see 3-I. **8-J. The `onward` channel is
unbounded.** Low, confidence 8. New (`input.rs:217, 258, 1011`). Under LP-188. **8-K.**
Concurrent starts: see 3-H. **8-L. `in_flight` round-trip entries are never pruned.** Info
(`input.rs:595-597`).

### 9 Secrets and material

**Status:** PARTIAL **Risk:** Medium **Change:** 9-C New

**Passes.** No private key, keystore, PEM or token in the tree or the history; `vectors/keys.json`
is labelled test material; only the release public key is in the repository
(`impl/crates/lpctl/src/upgrade.rs:31`); the minisign secret comes only from a workflow secret
in the release job and is never echoed (`.github/workflows/release.yml:183-194`); the Android
keystore only from the environment and secrets; no `info` or `warn` line logs typed text, key
codes, clipboard content, screen bytes, notification bodies or key material, in the daemon,
`lpview`, the node or the app.

**9-A. The release falls back to a throwaway APK signing key.** Medium, confidence 9.
Unchanged (`release.yml:145-162`): without the keystore secret the job generates one, warns,
and publishes, with the password fallbacks `android`. Under LP-190.

**9-B. File names and paths at `info`.** Low, confidence 9. Unchanged (`files_sync.rs:538,
1395, 1639`, `run.rs:119`, `daemon.rs:335`).

**9-C. The release signing key is a repository-wide secret with no environment gate.**
Medium, confidence 7. New. The secret is read in a job with no `environment`
(`release.yml:172-206`), the workflow runs on any pushed `v*` tag with the tagged commit's
own `release.yml`, and the key file in `$RUNNER_TEMP` is still present when
`softprops/action-gh-release` runs (`:183-201`). Anyone who can push a tag, or a stolen token
with `repo` and `workflow` scope, can tag a commit whose workflow prints the key, which is the
sole trust root of `lpctl upgrade`. Recommendation (LP-190): a `release` environment with
required reviewers and a tag ruleset; delete the key file before any third-party action;
consider signing offline, which `scripts/sign-release.sh` already allows.

### 10 Dependencies and build

**Status:** PARTIAL **Risk:** Medium **Change:** 10-A Resolved in part; 10-D to 10-G New

**Passes.** `cargo deny check` is clean (advisories, bans, licences, sources); the four
advisory ignores carry reasons (`impl/deny.toml:9-18`); `Cargo.lock` is tracked; `cargo-ndk`,
`rsign2`, `cargo-llvm-cov` and ajv are pinned; `site/build.sh` fetches mdBook and lychee by
version and SHA-256; `unsafe` appears only in `lp-macos` (50 blocks, 5 `unsafe fn`, every one
with a SAFETY comment or a `# Safety` section) and nowhere in `ligature-android`'s own code;
`lpctl upgrade` verifies in the right order: the signed `SHA256SUMS` against the compiled-in
key with the trusted comment bound to the version, then the tarball's hash, then unpack, then
`install.sh`, with sizes and timeouts bounded and only newer versions installed
(`upgrade.rs:163-186, 255-334`).

**10-A. Unsigned assets; quarantine stripped.** Resolved for upgrades through `lpctl`;
Low remaining, confidence 9: `SHA256SUMS` covers every asset and is signed
(`scripts/sign-release.sh:26-60`), but `install.sh` itself verifies nothing and still removes
`com.apple.quarantine` from ad hoc signed binaries (`packaging/install.sh:128-131`), so a first
install from a browser relies on the manual steps in the install guide, and binaries are not
notarised. Under LP-190.

**10-B. Write token in the build jobs; no `permissions` in `ci.yml`; actions on majors.**
Medium, confidence 8. Unchanged apart from `cargo-ndk`: `contents: write` at workflow level
(`release.yml:22-23`), `pages: write` and `id-token: write` on the Pages build job, no checkout
with `persist-credentials: false`, actions on mutable tags and branches. Under LP-190.

**10-C. `unsafe_code` overrides cover whole crates; SAFETY comments missing.** Low, confidence
8. Unchanged (`impl/crates/ligature-android/Cargo.toml:42`, `lp-macos/src/pasteboard.rs:83-86`);
the protocol conformances and `extern "C"` blocks of the new `lp-macos` files carry no comment
either (`notifications.rs:89-106`, `screen.rs:33, 173-193`, `input.rs:53-179`).

**10-D. A release without the minisign secret is still published, signed by a throwaway
key.** Low, confidence 8. New (`release.yml:188-194`, `sign-release.sh:46-55`): `lpctl
upgrade` refuses it, while the daily check announces it. **10-E. Release and Gradle builds
run without `--locked`.** Low, confidence 7. New (`release.yml:76, 99-100`,
`impl/android/app/build.gradle.kts:140-156`). **10-F. `lpctl upgrade` does not require HTTPS.**
Info, confidence 8. New (`upgrade.rs:361-367`, ureq defaults): integrity holds through the
signature; the version fetched is what leaks. **10-G. An unsigned listing can freeze
upgrades.** Low, confidence 7. New: `latest()` picks the highest tag before any check
(`upgrade.rs:115-140`), so a tag without a signature pins every device to a version that never
verifies. All four under LP-190.

### 11 Threat model coverage

**Status:** PARTIAL **Risk:** Medium (after the High's fix) **Change:** 11-A fixed; 11-B to
11-F New

| Chapter 09 goal | Status | Evidence or finding |
| --- | --- | --- |
| Network and helper confidentiality and integrity | Met on the LAN; helpers unimplemented | `session.rs` as under item 3 |
| No off-device party learns the device set or grants | Met | 1-B and 5-A resolved |
| Least exposure | Partly | 11-C: a chained drive passes keystrokes through the middle device |
| Authenticity of device membership | Met after LP-185 | 1-H |
| Forward secrecy, post-compromise security | Met | no tickets, no resumption |
| Leaked self-signing key | Open in the spec | 1-E, 2-E |
| Nothing before mutual authentication | Met for streams | 11-E for what a dial reveals |
| §4.4 input only to the person's own devices | Met | `input.rs:696-717, 902-907`; 11-F latent |
| §4.4 revocation ends a live drive or share | Met | `daemon.rs:885-899`, `input.rs:633-645`, `screen.rs:458-487` |
| §4.4 everything held lifted however a drive ends | Met, 5-F aside | `input.rs:667-685, 1124-1125` and each platform |
| §4.4 the phone driven only with its service on, which reads no screen content | Met in code | `drive_service.xml:5-11`; 11-D for what it is granted |
| §4.4 a share only to the same person, nothing stored | Met | `screen.rs:498-525, 595-599`; frames through pipes only |
| §4.4 the sharing device shows it shares for the share's life | Partly | macOS and Android by the system; Linux best effort, 5-D |
| §4.4 the phone asks for every share unless the `adb` grant is set | Met | `ScreenShare.kt:56-67, 84-110, 156-181` |
| §4.5 no remote trust | Met | no path makes a `person` role; a known person needs the owner's role |
| §5 discovery reveals no identity | Met for the record | 4-A deferred, 11-E |
| §5 files by base name, offers by announced id, cache cleared | Met | `files.rs:414-437`, `files_sync.rs:1240-1270`, `clipboard_sync.rs:146, 676-685` |
| Groups, shared devices, helpers, signed records, relay denial of service | Unimplemented layers | stubs only |

**11-A.** See 1-H. **11-B.** See 5-D.

**11-C. A chained drive passes keystrokes through an intermediate device.** Low, confidence
7. New. B decrypts A's events and relays them to C (`input.rs:886-889, 980-1013`), so B sees
keystrokes meant for C, against §4's least exposure among the person's own devices.
Recommendation: record the relay as an accepted exception in §4, or hand A a direct drive.

**11-D. The accessibility service is granted more than it uses.** Info, confidence 7. New.
Without `android:packageNames` it receives window-state events, which carry text, from every
package, and `flagInputMethodEditor` gives it an input connection; the code reads only
`event.text.size` and commits text. Recommendation: `packageNames="com.android.systemui"`, or
word the spec "does not read".

**11-E. Dialling on mDNS reveals the binding key to any advertiser.** Low, confidence 8. New
facet of §5: the daemon dials any `_ligature._udp` record (`run.rs:335-363`) and presents its
key after the handshake, so a fake advertiser collects every device's long-term key.
Recommendation: dial only `id`s mapped to known peers or rate-limit; record it in §5.

**11-F. The manifest, with input and screen, goes to every trusted peer.** Info, latent. New
facet of 5-B.

Spec claims the code does not implement, unchanged: the principal service, the headless
pairing page, groups, the macOS Keychain, notarised distribution, the staleness period and the
word-list code, abandonment and rebuild, every helper role and signed discovery records.

### 12 Injection and parsing

**Status:** PARTIAL **Risk:** Low **Change:** 12-G New

**Passes.** Every SQL statement binds its values; the two `format!`s add only placeholders or
static clauses (`trust_store.rs:407, 514`); the frame bound is one constant everywhere; input
and screen descriptors ignore unknown fields and require integers of at least 1
(`impl/crates/lp-core/src/screen.rs:203-237`, `impl/crates/lp-core/src/input.rs:54-71`); `keyid`
lookups are checked (`impl/crates/lp-core/src/input/keyid.rs:126-260`); `KeyFinder` uses
`try_from` and `checked_shl`; `text_keymap` names each character as `U%04X`, so peer text
cannot inject keymap syntax, with 240 distinct characters at most (`input_linux.rs:783-807`);
the encoder command lines take only clamped integers and fixed strings, and nothing goes
through a shell (`encode.rs:101-200`, `screen_linux.rs:589`); Android clamps coordinates and
wraps `dispatchGesture`; `upgrade.rs` caps the JSON, parses the tag as semver and passes fixed
arguments to `tar` and `sh`; `permissions.rs` parses only its own child's output.

**12-A. Peer names unbounded and printed raw.** Low, confidence 9. Unchanged and widened:
`display_name` has no length or character check (`impl/crates/lp-core/src/facts.rs:54, 113`);
`lpctl` prints device names, the sharer's display ids and names, `started.display` and
offered file names raw (`main.rs:665-670, 843-865, 973-981, 1006-1013, 1235`). **12-B. The
offer parser has no bounds.** Low, confidence 9. Unchanged (`pairing.rs:96-102`). **12-C.**
Info. Unchanged.

**12-G. Non-finite floats from a peer are accepted in input events.** Info, confidence 8. New.
`dx`, `dy`, `x`, `y` and `at` decode NaN and infinities (`lp-core/src/input.rs:197-243,
274-286`); `clamp` passes NaN, so a driven device's position stays NaN and an edge drive
cannot end by its edge (`input.rs:1000-1015, 1131`); nothing panics and the chord still works.
Recommendation: drop events with non-finite fields in `apply`.

### 13 Content passed through

**Status:** FAIL (13-A) **Risk:** Medium **Change:** 13-E, 13-F New

**Passes.** No markup is parsed or rendered anywhere: Android notifications are plain
strings, macOS notifications use `setTitle` and `setBody`, no `fromHtml`, `WebView` or
`Linkify` exists; icons are checked against their hash and bound before use
(`notification_sync.rs:283-289`) and named by the hash; the provider serves announced keys
only, read-only, not exported; the Mac posts text as UTF-16 chunks with no flags
(`impl/crates/lp-macos/src/input.rs:1089-1119`); the Android viewer decodes in the platform's
sandboxed codec process and drops oversize input.

**13-A. Linux notification bodies go to `Notify` without markup escaping.** Medium,
confidence 9. Unchanged (`notifications_linux.rs:164-173`): a body chosen by whoever messaged
the phone reaches the desktop's notification server as markup, and the received-file
notification carries the peer's name. Recommendation as before: escape `& < >`.

**13-B. The Wayland helper offers any MIME a peer sends.** Low, confidence 9. Unchanged
(`clipboard_linux.rs:498-511, 820-840`). **13-C.** Info. Unchanged.

**13-E. Peer display names and ids reach the terminal unfiltered.** Low, confidence 8. New
facet of 12-A: `Display` and `Started` carry free strings with no check
(`lp-core/src/screen.rs:301-318, 361-373`); they reach no path or command line, since the
sharer matches a requested id against its own list (`screen.rs:638-656`).

**13-F. A peer's H.264 stream is decoded by an unsandboxed ffmpeg child.** Low, confidence 8.
New. `lpview` runs `ffmpeg -f h264 … -i pipe:0` on the bytes the peer sends
(`impl/crates/lpview/src/main.rs:329-381, 737-741`), as the user, with no seccomp, Landlock,
namespace or rlimit; `-f h264` narrows the surface to one parser and the software decoder,
and a memory-safety bug there would be code execution as the user rather than a failed
decode. Recommendation: run the decoder under a sandbox with memory and CPU limits; pin
`-c:v h264` and `-threads`.

### 14 Security logging

**Status:** FAIL **Risk:** Medium **Change:** 14-C New

**Passes.** A stranger's input, screen, files, clipboard set or notification is logged at
`warn` with the peer (`input.rs:1112`, `screen.rs:596`, `files_sync.rs:1194, 1248`,
`clipboard_sync.rs:728`, `notification_sync.rs:405`); a missing Mac permission at `info` or
`warn` (`input_macos.rs:207-215`, `screen_macos.rs:154-160`); the clipboard promise's hash
mismatch at `info` with the peer; no line carries content.

**14-A. Security-relevant rejections silent or `debug` only.** Medium, confidence 9.
Unchanged: a file's hash or size mismatch and a declined files offer are not logged
(`files_sync.rs:1198-1209, 1442-1461, 1622-1624`); `ceremony.rs` has no `tracing` call at all;
an unknown purpose, a stream before admission and an invalid header are `debug`
(`manager.rs:261, 357, 390`); the discarded-fact line still lacks the peer
(`impl/crates/ligatured/src/gossip.rs:59`).

**14-B. Peer strings logged unescaped.** Low, confidence 7. Unchanged; the peer's display id
joins the list (`screen.rs:446, 586`).

**14-C. Screen refusals and remote refusals are not logged.** Low, confidence 8. New: the
sharer answers `Refused` with no log line (`screen.rs:623-648, 864-869`), a screen stream on a
device that does not share is reset silently (`:590-592`), and a drive or share another
device refused reaches only the control client (`input.rs:495-496`, `screen.rs:417-425`).
Recommendation: `info` with the peer and the reason at each refusal.

### 15 OWASP Top 10 cross-check

| Category | Where it lands | Result |
| --- | --- | --- |
| A01 Broken access control | items 1, 5 | met for the families; 1-H was an authority escalation, fixed; 11-F latent |
| A02 Cryptographic failures | items 2, 3, 10 | met; releases signed with minisign and verified by `lpctl upgrade` |
| A03 Injection | item 12 | met for SQL, CBOR and every command line; 12-A terminal escapes |
| A04 Insecure design | items 1, 11 | 1-E, 2-E, 2-F, 4-A and same-person implicit authority are the spec's open items |
| A05 Security misconfiguration | items 7, 10 | 7-E, 7-G, 9-C, 10-B, the fixed port (4-C), the accessibility config (11-D) |
| A06 Vulnerable components | item 10 | clean at this commit |
| A07 Authentication failures | item 1 | the ceremony holds; 1-D race; a stranger's wrong code aborts a join (15-E) |
| A08 Integrity failures | items 2, 10 | facts met after 1-H; 2-F open; `install.sh` verifies nothing on a first install (10-A) |
| A09 Logging failures | item 14 | 14-A, 14-C |
| A10 Server-side request forgery | items 4, 10 | the daemon dials what mDNS or an offer names (4-B, 11-E, 12-B); `lpctl upgrade` follows redirects to any host (10-F); no URL a peer supplies is fetched |

**15-E. The first pairing stream wins, so a stranger can abort a join.** Low, confidence 7.
New: a wrong code on the first `pairing` stream aborts the newcomer's ceremony
(`ceremony.rs:316-320`, `pairing.rs:416-418`); denial of service only. Recommendation: drop
that stream and keep waiting until a timeout.

## Other observations

- The automatic clipboard send on the phone forwards any app's clipboard write to every
  desktop while the accessibility service is on; that is the opt-in feature, with clips marked
  sensitive skipped.
- The Android unattended share also needs the accessibility service on, so a phone without it
  still asks every time, as the guide says.
- The accept loop completes handshakes one at a time and discovery dials one record at a time
  with a 5 s timeout: denial-of-service shapes by a stranger, out of scope, noted for the
  relay work.
- `ClipboardConfig::default()` still names a temporary directory, overridden by every caller.

## Critical findings

| Item | Status | Risk | Finding |
| --- | --- | --- | --- |
| 1, 2, 11 | Fixed, LP-185 (ADR 0047) | High | Any paired device could revoke the set by inventing a person (1-H) |
| 2, 3 | Open, LP-189 | Medium | A fact's identity ignores its signer; the first copy wins (2-F); a flood of facts silences gossip (3-B) |
| 5, 11 | Open, LP-187 | Medium | A Linux share can run with no indicator, and a viewer can close it (5-D) |
| 5, 8 | Open, LP-188 | Medium | A stalled peer defeats the local release and stop (5-E) |
| 8, 3, 12, 13 | Open, LP-186 | Medium | The viewer takes `started` and frames without bounds (8-F) |
| 9, 10 | Open, LP-190 | Medium | The release key in a repository-wide secret; write token in build jobs; throwaway keys (9-A, 9-C, 10-B) |
| 1 | Open | Medium | The offer holder's pairing race (1-D) |
| 6, 8 | Open | Medium | A clipboard fetch trusts the fetch-time offer (6-A) |
| 13 | Open | Medium | Linux notification bodies without markup escaping (13-A) |
| 14 | Open | Medium | Rejections silent or `debug` only (14-A) |

## Deferred or accepted risk

| Item | Risk | Deferred since | Reason | What would unblock it |
| --- | --- | --- | --- | --- |
| 1-E | Medium | 2026-10-23 | Every introducer holds the self-signing key by design (ADR 0031) | Self-signing rotation, or introducer opt-in with a rebuild on revoking one |
| 4-A | Medium | 2026-10-23 | The raw binding key is the TLS identity (ADR 0028); the per-epoch identifier is open | The per-epoch identifier, or proving the key after recognition |
| 7-F | Info | 2026-10-23 | Device security is out of scope (ADR 0024); the reach is now every device of the person's | A revisit of ADR 0024 for `pair`, `input.drive` and `screen.start` |
| 7-H | Info | 2026-11-08 | The unattended phone share is the owner's choice over `adb` (LP-167), documented | A stop control while driven |
| 6-I | Info | 2026-10-23 | Pushes between a person's own devices are accepted by design | A free-space check |
| 11-C | Low | 2026-11-08 | A chained drive relays through the middle device by design (LP-174) | A direct drive handed to the controller, or an exception recorded in chapter 09 §4 |

## Recommendations in priority order

1. Release the High fix (LP-185) in the next alpha, with the pre-release gate run against
   this report; its tests cover the forged person, a known person's role over the owner's
   device and a stranger revoking their own.
2. Before or with the release, the two Mediums a same-person device reaches on the new
   families: bound the viewer (LP-186) and make the Linux share indicator mandatory and
   unclosable by a peer (LP-187).
3. The remaining new Mediums: the local release and stop under a stalled peer, with the
   bounded queues (LP-188); the release pipeline's tokens, secret gating and `--locked` builds
   (LP-190); the fact identity and store growth, which need an ADR (LP-189).
4. The Mediums carried from the previous audit: the fetch-time offer (6-A), notification
   markup escaping (13-A), the pairing race (1-D), the silent rejections (14-A).
5. Low and Info as listed, in whatever order the milestones touch that code; the vectors for
   2-D's gaps alongside LP-189.
