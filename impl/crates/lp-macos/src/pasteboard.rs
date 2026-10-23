//! `NSPasteboard` through `objc2`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType,
    NSPasteboardWriting,
};
use objc2_foundation::{
    NSArray, NSData, NSDate, NSDefaultRunLoopMode, NSObject, NSObjectProtocol, NSRunLoop, NSString,
    NSURL,
};

/// What a provider answers for a lazily offered type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provided {
    /// The bytes of the type.
    Data(Vec<u8>),
    /// A file whose URL is the value of the type (`public.file-url`).
    FileUrl(PathBuf),
}

/// Answers a request for a lazily offered type, by UTI. Called on whatever
/// thread the pasteboard uses; may take as long as a fetch.
pub type Provider = Arc<dyn Fn(&str) -> Option<Provided> + Send + Sync>;

/// The pasteboard's current value, as [`read`] returns it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadValue {
    /// The string of the text type asked for.
    pub text: Option<String>,
    /// The data of each data type asked for that is present, by UTI.
    pub data: Vec<(String, Vec<u8>)>,
    /// The local paths of the file URLs on the pasteboard, item by item.
    pub file_paths: Vec<PathBuf>,
}

/// One pasteboard item to write.
#[derive(Clone, Default)]
pub struct WriteItem {
    /// A string, with its type.
    pub text: Option<(String, String)>,
    /// Data by type, written at once.
    pub data: Vec<(String, Vec<u8>)>,
    /// Types offered lazily: the item's provider is asked when an
    /// application wants one.
    pub lazy: Vec<String>,
    /// Answers the lazy types; required when there are any.
    pub provider: Option<Provider>,
}

impl std::fmt::Debug for WriteItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteItem")
            .field("text", &self.text.as_ref().map(|(u, _)| u))
            .field(
                "data",
                &self.data.iter().map(|(u, _)| u).collect::<Vec<_>>(),
            )
            .field("lazy", &self.lazy)
            .finish()
    }
}

/// What a data provider serves: the provider function and the types it was
/// registered for.
struct ProviderIvars {
    provider: Provider,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "LigaturePasteboardProvider"]
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
            let Some(provided) = (self.ivars().provider)(&uti) else {
                return;
            };
            match provided {
                Provided::Data(bytes) => {
                    item.setData_forType(&NSData::with_bytes(&bytes), r#type);
                }
                Provided::FileUrl(path) => {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                    if let Some(s) = url.absoluteString() {
                        item.setString_forType(&s, r#type);
                    }
                }
            }
        }
    }
);

impl DataProvider {
    fn new(provider: Provider) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ProviderIvars { provider });
        // SAFETY: NSObject's init takes no arguments.
        unsafe { msg_send![super(this), init] }
    }
}

/// A provider kept alive while its value stands on the pasteboard.
struct Keep(#[allow(dead_code)] Retained<DataProvider>);

// SAFETY: the provider is only ever called by the pasteboard and holds a
// `Send + Sync` function; the retained pointer itself is moved between
// threads only to be dropped.
unsafe impl Send for Keep {}

static PROVIDERS: Mutex<Vec<Keep>> = Mutex::new(Vec::new());

/// The pasteboard's change count, which moves on every write.
#[must_use]
pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

/// Reads the pasteboard: the string of `text_uti`, the data of each of
/// `data_utis` that is present, and the file URLs of every item.
#[must_use]
pub fn read(text_uti: &str, data_utis: &[&str], file_url_uti: &str) -> ReadValue {
    let pb = NSPasteboard::generalPasteboard();
    let mut value = ReadValue {
        text: pb
            .stringForType(&NSString::from_str(text_uti))
            .map(|s| s.to_string()),
        ..ReadValue::default()
    };
    for uti in data_utis {
        if let Some(data) = pb.dataForType(&NSString::from_str(uti)) {
            let bytes = data.to_vec();
            if !bytes.is_empty() {
                value.data.push(((*uti).to_owned(), bytes));
            }
        }
    }
    if let Some(items) = pb.pasteboardItems() {
        let file_url = NSString::from_str(file_url_uti);
        for item in items.iter() {
            let Some(s) = item.stringForType(&file_url) else {
                continue;
            };
            let Some(url) = NSURL::URLWithString(&s) else {
                continue;
            };
            if let Some(path) = url.path() {
                value.file_paths.push(PathBuf::from(path.to_string()));
            }
        }
    }
    value
}

/// Replaces the pasteboard's content with `items`, each lazy type answered
/// by its item's provider, and returns the new change count.
///
/// # Errors
///
/// Returns the pasteboard's refusal as text, or an item with lazy types and
/// no provider.
pub fn write(items: &[WriteItem]) -> Result<isize, String> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let mut keep = Vec::new();
    let mut objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
    for item in items {
        let pb_item = NSPasteboardItem::new();
        if let Some((uti, text)) = &item.text
            && !pb_item.setString_forType(&NSString::from_str(text), &NSString::from_str(uti))
        {
            return Err(format!("the pasteboard refused the string of {uti}"));
        }
        for (uti, bytes) in &item.data {
            pb_item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(uti));
        }
        if !item.lazy.is_empty() {
            let provider = item
                .provider
                .clone()
                .ok_or_else(|| "lazy types without a provider".to_owned())?;
            let types: Vec<Retained<NSString>> =
                item.lazy.iter().map(|u| NSString::from_str(u)).collect();
            let data_provider = DataProvider::new(provider);
            pb_item.setDataProvider_forTypes(
                ProtocolObject::from_ref(&*data_provider),
                &NSArray::from_retained_slice(&types),
            );
            keep.push(Keep(data_provider));
        }
        objects.push(ProtocolObject::from_retained(pb_item));
    }
    if !pb.writeObjects(&NSArray::from_retained_slice(&objects)) {
        return Err("the pasteboard refused the items".to_owned());
    }
    *PROVIDERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = keep;
    Ok(pb.changeCount())
}

/// Runs the main thread's run loop for ever: the pasteboard delivers a
/// request for a lazily offered type to the owning process's main thread,
/// so a daemon whose main thread never runs the loop never answers one. The
/// daemon runs its own work on another thread and parks its main thread
/// here. With no input source yet the loop returns at once, so it is rerun
/// with a pause.
pub fn run_main_loop() -> ! {
    let run_loop = NSRunLoop::mainRunLoop();
    loop {
        let until = NSDate::dateWithTimeIntervalSinceNow(1.0);
        if !run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &until) {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}
