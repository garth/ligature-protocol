# Milestone 5 implementation decisions

Choices that are not part of the protocol but that a clipboard of several representations
needs settled. Each is recorded under the ticket that made it; the protocol-level choices
are in [05-rich-clipboard.md](05-rich-clipboard.md) section 3.

## LP-107: the daemon core, `lpctl` and the two-daemon test

**A value, not a string.** `ligatured::clipboard::Value` is what the local clipboard holds
as the platform read it: the plain text, the other typed parts with their bytes, and a
selection of files on this device's disk (`LocalFile`: path, base name, size, type, time).
A platform reports a `Change` carrying as much of the value as it could read; the Linux
helper, the Mac and the phone report text only until LP-108 and LP-109 teach them more.
The `Clipboard` trait keeps `read_text` and `set_text` and gains `read_value`, `set_value`,
`mime_types` and `holds_promises`, each defaulting to the text behaviour, so a text-only
platform still implements only the milestone 1 methods.

**What is set carries promises.** `Incoming` is what a peer sent, as the daemon puts it on
the local clipboard: the text, the pushed parts with bytes, the announced parts as
`Promise`s (type and descriptor) and the promised files, with a `Fetch` the platform calls
when an application asks for a promised type. The fetch is asynchronous (`FetchFuture`);
a platform thread blocks on it through the runtime's handle, and the in-memory fake awaits
it directly. A second request for the same id is answered from what the first fetched.

**The fingerprint is the echo rule.** `Fingerprint` maps each representation (`text/plain`
for the text, the type for a part, `file:<name>` for a file) to a SHA-256, from the bytes
for a local value and for a pushed part, from the announced hash for an announced part. The
echo filter keeps the fingerprint of the last value set and the last value sent, and a local
change whose fingerprint is within the last set (every type it reports hashes the same) is
the echo of our own set, so a platform that reads back fewer types than were set, such as
the phone reporting text only, never re-sends. A `set` equal to what was last sent is not
applied.

**One `set` per receiver.** `ClipboardSync::build_for` takes the receiver's params
(`ClipboardParams::from_descriptor`, text only when the peer advertises nothing new): the
text goes as `text` when it fits the push bound, parts the receiver lists go largest first
while the budget lasts, the rest are announced when the receiver has `fetch` and the daemon
has `FilesSync` to offer through, and files are announced on the same condition. A text
above the bound is announced as a `text/plain` part. The announced bytes are bounded by
`ClipboardConfig::keep_bytes` (64 MiB), the largest dropped first. Offers are registered
once per local value and reused for every receiver, under the part's type or `file:<n>`,
with the descriptor named `clipboard.<ext>` for a part and the file's own name for a file,
and withdrawn when the local value changes or `ClipboardConfig::window` (one hour) passes.

**The fetch is a `files` transfer into memory or the cache.** The `Fetcher` of an applied
value asks `FilesSync::fetch` for one id with `Destination::Memory` for a part up to
`ClipboardConfig::memory_limit` (16 MiB) and `Destination::Path` under the cache directory
otherwise and for every file, waits for the transfer, takes what landed from the transfer
(`Transfer::take_landed`, new: a received file's `Landed`, bytes included for memory), and
checks the landed hash against the announced one. A fetch that fails for any reason, a
mismatch, or the sender disconnecting drops the value's promises: the clipboard is set
again to the pushed parts alone, with the fingerprint noted so that set is not an echo.

**The cache is per value and cleared on change.** `ClipboardConfig::cache_dir` is
`$XDG_RUNTIME_DIR/ligature/clipboard` on Linux,
`~/Library/Caches/org.ligatureprotocol.ligatured/clipboard` on macOS and the temporary
directory elsewhere; each applied value gets a numbered directory under it, removed when a
newer value replaces it, and the whole cache is removed when the synchroniser starts.

**`get` on connect, answered with patience.** On attaching a same-person peer the stream
opens with a `get`; the answer is built for the asker like any `set`, or empty when there
is no local value. A `get` tends to arrive before the asker's manifest and before the
answerer's own stream to it is open, so the answer waits up to three seconds for both. An
empty `set` is never applied, so a device that holds nothing does not clear a peer's
clipboard. The local watcher subscribes before anything else runs and the synchroniser
seeds its local value from what the clipboard holds at start, so a daemon that restarts
can answer at once. `ClipboardSync::request` sends a `get` on demand, for the phone's
**Receive now** (LP-109).

**Counters count content.** `counters()` counts `set` messages with content sent and
applied; empty answers to `get` are not counted, which keeps the milestone 1 test exact.

**Types in `lp-core`.** `ClipboardSet` has an optional `text`, `parts` and `files`;
`ClipboardPart` is pushed (`bytes`) or announced (`file`); `ClipboardGet` and the untagged
`ClipboardMessage` decode the stream by shape; `ClipboardParams` carries the descriptor's
params and `clipboard_descriptor_with` builds the descriptor, while the milestone 1
`clipboard_descriptor` stays for the conformance vectors. The fingerprint lives in
`ligatured`, since hashing is `lp-crypto`'s and `lp-core` does not depend on it.

**`lpctl clipboard get`** prints the text and, on standard error, each other part with its
type and size and whether it is a promise, and each promised file; `set` still sets text.
