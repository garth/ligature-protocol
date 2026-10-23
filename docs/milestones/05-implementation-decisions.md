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

## LP-108: the desktop clipboards

**The Linux helper owns a data-control source.** `wl-clipboard-rs` reads a selection and
copies bytes it holds, but cannot serve a type on request, so the helper
(`ligatured clipboard-helper`) keeps its `Watcher` for reading and adds a second Wayland
connection of its own through `wayland-client`, binding `ext-data-control` or, failing
that, `wlr-data-control`, with one data device per seat. A `set` from the daemon becomes a
source offering the pushed parts (a text under the five plain-text names applications ask
for) and the promised types; a `send` for a pushed type writes the bytes from the helper's
copy, and one for a promised type asks the daemon (`fetch` over the helper's stdio, matched
by id, waited for up to two minutes) and writes what comes back, in a thread per request
with `O_NONBLOCK` cleared. The source loop polls the connection and a wake pipe, so a `set`
from the daemon interrupts it. A promised file list is one promised `text/uri-list` whose
fetch brings every file into the cache and answers with their `file:` URIs; a `text/uri-list`
read from another application becomes the value's files when its URIs are local paths.

**The helper tells its own selection apart by the offered types.** The compositor reports
the helper's own selection to its watcher like any other; the helper skips a change whose
set of types equals what it offered, and reads again once its source is cancelled. Frames
between the daemon and the helper are bounded at 256 MiB rather than the wire's 1 MiB,
since a clipboard value is not a wire message.

**The Mac keeps the pasteboard's own laziness.** `MacClipboard` maps types through a table
(`public.html`, `public.rtf`, `public.png`, `public.tiff`, with `public.utf8-plain-text` for
the text and `public.file-url` for files), writes a value as `NSPasteboardItem`s (the text
and pushed parts as data, a lazy `NSPasteboardItemDataProvider` for the announced types, and
one item per promised file whose `public.file-url` provider fetches the file into the cache
and answers with its URL), and skips its own `changeCount` when polling. The provider is an
Objective-C class defined with `objc2::define_class!` holding the value's fetcher and a
runtime handle it blocks on. The plan's `NSFilePromiseProvider` is not used: a fetched file
URL is enough for Finder and needs no completion blocks. This code is checked by CI's macOS
build and the run, since the Linux machine cannot compile for the Mac.

**`lpctl clipboard put` and `paste`** exist for the run: `put <mime> <path>` puts one typed
part on the local clipboard as if copied, and `paste <mime> <out>` has the daemon write what
an application would get, fetching a promise, to a file, since a value can exceed a control
frame. The fake clipboard's paste fetches; a platform clipboard's answers from the pushed
parts, as its promises are served to applications by the platform.

**A nested compositor for the run.** The desktop checks ran against a nested Hyprland on its
own socket (`Hyprland --socket lp-test --wayland-fd N` with a socket handed over), so the
machine's real clipboard and the owner's daemon were untouched; `LIGATURE_CLIPBOARD_CACHE`
names the cache directory so two daemons on one machine do not share one.
