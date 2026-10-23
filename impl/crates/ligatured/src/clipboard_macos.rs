//! The macOS clipboard through `NSPasteboard`. Milestone 1 decision LP-29,
//! milestone 5 LP-108; spec chapter 08 section 2b.
//!
//! The pasteboard has no change notification, so a thread polls
//! `changeCount` every [`POLL`] and reads the value when it moves and the
//! change is not this daemon's own write. The interval is chosen against the
//! one-second exit criterion: a change is seen within a quarter second on
//! average and costs one integer read per poll.
//!
//! Types map through a small table between MIME types and UTIs. A value a
//! peer sent is written as one `NSPasteboardItem` carrying the text and the
//! pushed parts as data, with a lazy data provider for each announced type
//! that fetches the bytes when an application asks, and one item per
//! promised file whose `public.file-url` provider fetches the file into the
//! cache and answers with its URL, which Finder copies from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lp_core::Error;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType,
    NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSObject, NSObjectProtocol, NSString, NSURL};
use tokio::sync::broadcast;

use crate::clipboard::{Change, Clipboard, Fetch, Incoming, Part, TEXT_PLAIN, Value};

/// How often `changeCount` is read.
pub const POLL: Duration = Duration::from_millis(250);

/// The UTI of plain UTF-8 text, the value of `NSPasteboardTypeString`.
const UTF8_TEXT: &str = "public.utf8-plain-text";
/// The UTI of a file URL, the value of `NSPasteboardTypeFileURL`.
const FILE_URL: &str = "public.file-url";

/// MIME types and their UTIs, in the order they are read.
const TABLE: [(&str, &str); 4] = [
    ("text/html", "public.html"),
    ("text/rtf", "public.rtf"),
    ("image/png", "public.png"),
    ("image/tiff", "public.tiff"),
];

fn uti_for(mime: &str) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(mime))
        .map(|(_, u)| *u)
}

/// What a data provider serves: the fetcher of the value it belongs to and
/// the id behind each UTI it was registered for.
struct ProviderIvars {
    fetch: Option<Arc<dyn Fetch>>,
    ids: HashMap<String, String>,
    runtime: tokio::runtime::Handle,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "LigatureClipboardProvider"]
    #[ivars = ProviderIvars]
    struct DataProvider;

    unsafe impl NSObjectProtocol for DataProvider {}

    unsafe impl NSPasteboardItemDataProvider for DataProvider {
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn provide(
            &self,
            _pasteboard: Option<&NSPasteboard>,
            item: &NSPasteboardItem,
            r#type: &NSPasteboardType,
        ) {
            let uti = r#type.to_string();
            let ivars = self.ivars();
            let (Some(id), Some(fetch)) = (ivars.ids.get(&uti), ivars.fetch.as_ref()) else {
                return;
            };
            let fetched = match ivars.runtime.block_on(fetch.fetch(id)) {
                Ok(f) => f,
                Err(e) => {
                    tracing::debug!(%e, uti, "cannot fetch a promised clipboard type");
                    return;
                }
            };
            if uti == FILE_URL {
                let Some(path) = fetched.path else { return };
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                if let Some(s) = url.absoluteString() {
                    item.setString_forType(&s, r#type);
                }
                return;
            }
            let bytes = match (fetched.bytes, fetched.path) {
                (Some(b), _) => b.as_ref().clone(),
                (None, Some(p)) => match std::fs::read(&p) {
                    Ok(b) => b,
                    Err(_) => return,
                },
                (None, None) => return,
            };
            item.setData_forType(&NSData::with_bytes(&bytes), r#type);
        }
    }
);

impl DataProvider {
    fn new(ivars: ProviderIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        // SAFETY: NSObject's init takes no arguments.
        unsafe { msg_send![super(this), init] }
    }
}

/// The macOS clipboard.
pub struct MacClipboard {
    last: Arc<Mutex<Option<Value>>>,
    /// The change count of this daemon's own last write, not reported back.
    own_count: Arc<Mutex<isize>>,
    /// The providers of the current value, kept while it stands.
    providers: Mutex<Vec<Retained<DataProvider>>>,
    changes: broadcast::Sender<Change>,
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for MacClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacClipboard")
    }
}

// SAFETY: the Objective-C objects held are only ever used through the
// pasteboard, which takes calls from any thread.
unsafe impl Send for MacClipboard {}
unsafe impl Sync for MacClipboard {}

/// Reads the pasteboard's current value: the text, the parts of the table's
/// types, and the file URLs as local files.
fn read_pasteboard() -> Option<Value> {
    let pb = NSPasteboard::generalPasteboard();
    let mut value = Value {
        text: pb
            .stringForType(&NSString::from_str(UTF8_TEXT))
            .map(|s| s.to_string()),
        parts: Vec::new(),
        files: Vec::new(),
    };
    for (mime, uti) in TABLE {
        if let Some(data) = pb.dataForType(&NSString::from_str(uti)) {
            let bytes = data.to_vec();
            if !bytes.is_empty() {
                value.parts.push(Part::new(mime, bytes));
            }
        }
    }
    if let Some(items) = pb.pasteboardItems() {
        let file_url = NSString::from_str(FILE_URL);
        for item in items.iter() {
            let Some(s) = item.stringForType(&file_url) else {
                continue;
            };
            let Some(url) = NSURL::URLWithString(&s) else {
                continue;
            };
            let Some(path) = url.path() else { continue };
            if let Ok(f) =
                crate::clipboard_sync::local_file(std::path::Path::new(&path.to_string()))
            {
                value.files.push(f);
            }
        }
    }
    if value.is_empty() { None } else { Some(value) }
}

impl MacClipboard {
    /// Starts the polling thread and returns the clipboard. Called within
    /// the daemon's runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let (changes, _) = broadcast::channel(32);
        let this = Arc::new(Self {
            last: Arc::new(Mutex::new(None)),
            own_count: Arc::new(Mutex::new(-1)),
            providers: Mutex::new(Vec::new()),
            changes,
            runtime: tokio::runtime::Handle::current(),
        });
        let last = Arc::clone(&this.last);
        let own = Arc::clone(&this.own_count);
        let tx = this.changes.clone();
        std::thread::Builder::new()
            .name("pasteboard-poll".into())
            .spawn(move || {
                let pb = NSPasteboard::generalPasteboard();
                let mut seen = pb.changeCount();
                loop {
                    std::thread::sleep(POLL);
                    let now = pb.changeCount();
                    if now == seen {
                        continue;
                    }
                    seen = now;
                    if *lock(&own) == now {
                        continue;
                    }
                    if let Some(value) = read_pasteboard() {
                        *lock(&last) = Some(value.clone());
                        let _ = tx.send(Change { value });
                    }
                }
            })
            .expect("spawn pasteboard thread");
        this
    }
}

impl Clipboard for MacClipboard {
    fn available(&self) -> bool {
        true
    }

    fn read_text(&self) -> Result<Option<String>, Error> {
        Ok(read_pasteboard().and_then(|v| v.text))
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        self.set_value(&Incoming::text(text))
    }

    fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn read_value(&self) -> Result<Option<Value>, Error> {
        Ok(read_pasteboard())
    }

    fn set_value(&self, value: &Incoming) -> Result<(), Error> {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let mut providers = Vec::new();
        let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
        let item = NSPasteboardItem::new();
        if let Some(t) = &value.text
            && !item.setString_forType(&NSString::from_str(t), &NSString::from_str(UTF8_TEXT))
        {
            return Err(Error::Io("NSPasteboard refused the string".to_owned()));
        }
        for p in &value.parts {
            if let Some(uti) = uti_for(&p.mime) {
                item.setData_forType(&NSData::with_bytes(&p.bytes), &NSString::from_str(uti));
            }
        }
        let promised: HashMap<String, String> = value
            .promises
            .iter()
            .filter_map(|p| uti_for(&p.mime).map(|u| (u.to_owned(), p.file.id.clone())))
            .collect();
        if !promised.is_empty() {
            let utis: Vec<Retained<NSString>> =
                promised.keys().map(|u| NSString::from_str(u)).collect();
            let provider = DataProvider::new(ProviderIvars {
                fetch: value.fetch.clone(),
                ids: promised,
                runtime: self.runtime.clone(),
            });
            item.setDataProvider_forTypes(
                ProtocolObject::from_ref(&*provider),
                &NSArray::from_retained_slice(&utis),
            );
            providers.push(provider);
        }
        items.push(ProtocolObject::from_retained(item));
        for f in &value.files {
            let item = NSPasteboardItem::new();
            let provider = DataProvider::new(ProviderIvars {
                fetch: value.fetch.clone(),
                ids: HashMap::from([(FILE_URL.to_owned(), f.id.clone())]),
                runtime: self.runtime.clone(),
            });
            item.setDataProvider_forTypes(
                ProtocolObject::from_ref(&*provider),
                &NSArray::from_retained_slice(&[NSString::from_str(FILE_URL)]),
            );
            providers.push(provider);
            items.push(ProtocolObject::from_retained(item));
        }
        if !pb.writeObjects(&NSArray::from_retained_slice(&items)) {
            return Err(Error::Io("NSPasteboard refused the items".to_owned()));
        }
        *lock(&self.own_count) = pb.changeCount();
        *lock(&self.providers) = providers;
        *lock(&self.last) = Some(Value {
            text: value.text.clone(),
            parts: value.parts.clone(),
            files: Vec::new(),
        });
        Ok(())
    }

    fn mime_types(&self) -> Vec<String> {
        std::iter::once(TEXT_PLAIN)
            .chain(TABLE.iter().map(|(m, _)| *m))
            .chain(std::iter::once("text/uri-list"))
            .map(str::to_owned)
            .collect()
    }

    fn holds_promises(&self) -> bool {
        true
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
