//! ScreenCaptureKit (milestone 8, LP-150): a display, or a window (milestone
//! 9, LP-201), captured as BGRA frames of a given size and rate, with the
//! system's sound as 32-bit float samples when asked, the displays and the
//! windows, and the Screen Recording permission the capture needs.
//!
//! A stream is a `SCStream` with an output object of this crate's own class,
//! whose frames and sound arrive on a dispatch queue of their own; each
//! frame's pixels are copied out tightly packed and handed to the caller's
//! closure, each sound buffer's samples interleaved and handed to another.
//! The stream ends when it is dropped, or on its own when the user stops it
//! from the menu bar's sharing indicator or the system fails it, which the
//! caller's last closure is told.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_audio_types::{
    AudioBufferList, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
    kAudioFormatLinearPCM,
};
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime,
    CMTimeFlags,
};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{
    NSArray, NSError, NSObject, NSObjectProtocol, NSOperatingSystemVersion, NSProcessInfo,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamErrorCode, SCStreamOutput, SCStreamOutputType, SCWindow,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut core::ffi::c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut core::ffi::c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut core::ffi::c_void) -> usize;
    fn CGDisplayModeRelease(mode: *mut core::ffi::c_void);
}

/// The display's size in backing pixels. `CGDisplayPixelsWide` and `High`
/// answer in points on a Retina display, whatever their names, so a scale-2
/// screen came out at half its size and scale 1 (LP-182); the current display
/// mode knows the pixels. Falls back to them when there is no mode.
fn pixel_size(display: u32) -> (usize, usize) {
    // SAFETY: plain queries on an id the system just listed; the mode is a
    // CoreFoundation object this function owns until it releases it.
    unsafe {
        let mode = CGDisplayCopyDisplayMode(display);
        if !mode.is_null() {
            let size = (
                CGDisplayModeGetPixelWidth(mode),
                CGDisplayModeGetPixelHeight(mode),
            );
            CGDisplayModeRelease(mode);
            if size.0 > 0 && size.1 > 0 {
                return size;
            }
        }
        (CGDisplayPixelsWide(display), CGDisplayPixelsHigh(display))
    }
}

/// How long ScreenCaptureKit has to list the displays or start a stream.
const ANSWER: Duration = Duration::from_secs(10);

/// Whether the process may record the screen.
#[must_use]
pub fn screen_access() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Asks for Screen Recording: macOS prompts once, and the answer takes
/// effect for the next process, so the daemon is restarted after allowing.
pub fn request_screen_access() {
    // SAFETY: a plain request with no arguments.
    unsafe {
        CGRequestScreenCaptureAccess();
    }
}

/// One display that can be shared.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenDisplay {
    /// The display's `CGDirectDisplayID`.
    pub id: u32,
    /// Its rectangle in the global space, in points.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width in points.
    pub width: f64,
    /// Height in points.
    pub height: f64,
    /// Width in pixels.
    pub pixel_width: u32,
    /// Height in pixels.
    pub pixel_height: u32,
    /// The display with the menu bar.
    pub main: bool,
    /// The built-in panel of a laptop.
    pub builtin: bool,
}

/// The active displays.
#[must_use]
pub fn screen_displays() -> Vec<ScreenDisplay> {
    let main = crate::input::main_display();
    crate::input::display_list()
        .into_iter()
        .map(|(id, r)| {
            let (pixel_width, pixel_height) = pixel_size(id);
            // SAFETY: a plain query on an id the system just listed.
            let builtin = unsafe { CGDisplayIsBuiltin(id) != 0 };
            ScreenDisplay {
                id,
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
                pixel_width: u32::try_from(pixel_width).unwrap_or(0),
                pixel_height: u32::try_from(pixel_height).unwrap_or(0),
                main: id == main,
                builtin,
            }
        })
        .collect()
}

/// One window that can be shared, as ScreenCaptureKit lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenWindow {
    /// The window's `CGWindowID`.
    pub id: u32,
    /// Its title, when it has one.
    pub title: Option<String>,
    /// The application that owns it, by name.
    pub app: Option<String>,
    /// Its width in points.
    pub width: f64,
    /// Its height in points.
    pub height: f64,
}

/// The shareable content, listed by ScreenCaptureKit within [`ANSWER`].
fn shareable_content() -> Result<Retained<SCShareableContent>, StartError> {
    let (tx, rx) = mpsc::channel::<Handed<Result<Retained<SCShareableContent>, (isize, String)>>>();
    let listed = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: a non-null content is live for the handler; retaining it
            // keeps it for the waiting thread.
            let result = match unsafe { Retained::retain(content) } {
                Some(c) => Ok(c),
                None => Err(error_text(error).unwrap_or((0, "no shareable content".to_owned()))),
            };
            let _ = tx.send(Handed(result));
        },
    );
    // SAFETY: a class method taking a completion handler, which it calls once.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&listed) };
    match rx.recv_timeout(ANSWER) {
        Ok(Handed(Ok(c))) => Ok(c),
        Ok(Handed(Err((code, text)))) => Err(if code == SCStreamErrorCode::UserDeclined.0 {
            StartError::Consent
        } else {
            StartError::Failed(text)
        }),
        Err(_) => Err(StartError::Failed(
            "the display list did not come".to_owned(),
        )),
    }
}

/// The windows on screen that can be shared: those with a title, in the
/// normal layer, with the application that owns each. Empty without Screen
/// Recording, which the listing needs too.
#[must_use]
pub fn screen_windows() -> Vec<ScreenWindow> {
    if !screen_access() {
        return Vec::new();
    }
    let Ok(content) = shareable_content() else {
        return Vec::new();
    };
    // SAFETY: the content's windows are live objects it owns, read here.
    unsafe {
        content
            .windows()
            .iter()
            .filter(|w| w.isOnScreen() && w.windowLayer() == 0)
            .filter_map(|w| {
                let title = w.title().map(|t| t.to_string()).filter(|t| !t.is_empty())?;
                let frame = w.frame();
                Some(ScreenWindow {
                    id: w.windowID(),
                    title: Some(title),
                    app: w
                        .owningApplication()
                        .map(|a| a.applicationName().to_string()),
                    width: frame.size.width,
                    height: frame.size.height,
                })
            })
            .collect()
    }
}

/// Whether this macOS captures the system's sound with the picture: macOS 13
/// and later have `capturesAudio`.
#[must_use]
pub fn audio_supported() -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: 13,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// What a stream captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A whole display, by `CGDirectDisplayID`.
    Display(u32),
    /// One window, by `CGWindowID`, fitted into the picture with borders.
    Window(u32),
}

/// Why a stream did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// Screen Recording is not allowed.
    Consent,
    /// No display or window has that id.
    NoDisplay,
    /// ScreenCaptureKit refused, with its message.
    Failed(String),
}

/// Why a running stream ended on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The user stopped it from the sharing indicator.
    User,
    /// The system ended it, with its message.
    Failed(String),
}

/// One frame: BGRA pixels, rows packed with no padding.
pub type OnFrame = Box<dyn Fn(&[u8], u32, u32) + Send + Sync>;
/// One buffer of sound: 32-bit float samples at 48 kHz, the channels
/// interleaved.
pub type OnAudio = Box<dyn Fn(&[f32]) + Send + Sync>;
/// Told once when the stream ends on its own.
pub type OnEnd = Box<dyn Fn(Ended) + Send + Sync>;

struct OutputIvars {
    on_frame: OnFrame,
    on_audio: Option<OnAudio>,
    on_end: OnEnd,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "LigatureScreenOutput"]
    #[ivars = OutputIvars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind == SCStreamOutputType::Audio {
                if let (Some(on_audio), Some(samples)) =
                    (self.ivars().on_audio.as_ref(), copy_samples(sample))
                {
                    on_audio(&samples);
                }
                return;
            }
            if kind != SCStreamOutputType::Screen {
                return;
            }
            if let Some((pixels, width, height)) = copy_pixels(sample) {
                (self.ivars().on_frame)(&pixels, width, height);
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let ended = if error.code() == SCStreamErrorCode::UserStopped.0 {
                Ended::User
            } else {
                Ended::Failed(error.localizedDescription().to_string())
            };
            (self.ivars().on_end)(ended);
        }
    }
);

impl Output {
    fn new(on_frame: OnFrame, on_audio: Option<OnAudio>, on_end: OnEnd) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            on_frame,
            on_audio,
            on_end,
        });
        // SAFETY: NSObject's init takes no arguments.
        unsafe { msg_send![super(this), init] }
    }
}

/// `kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment`.
const AUDIO_BUFFER_LIST_ALIGNED: u32 = 1;

/// The sound buffer's samples, interleaved: `None` for a buffer that is not
/// 32-bit float linear PCM, which ScreenCaptureKit's always is.
pub(crate) fn copy_samples(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
    // SAFETY: the sample is the framework's for the duration of the call; the
    // buffer list is sized as the framework asks, the block buffer it retains
    // is released here, and every read stays within the buffers it names.
    unsafe {
        let description = sample.format_description()?;
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&description);
        if asbd.is_null() {
            return None;
        }
        let asbd = *asbd;
        if asbd.mFormatID != kAudioFormatLinearPCM
            || asbd.mFormatFlags & kAudioFormatFlagIsFloat == 0
            || asbd.mBitsPerChannel != 32
        {
            return None;
        }
        let planar = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
        let channels = asbd.mChannelsPerFrame.max(1) as usize;
        let mut needed = 0usize;
        sample.audio_buffer_list_with_retained_block_buffer(
            &mut needed,
            std::ptr::null_mut(),
            0,
            None,
            None,
            AUDIO_BUFFER_LIST_ALIGNED,
            std::ptr::null_mut(),
        );
        if needed < std::mem::size_of::<AudioBufferList>() {
            return None;
        }
        // Room for the list and 16-byte alignment, as the flag asks.
        let mut storage = vec![0u64; needed.div_ceil(8) + 2];
        let base = storage.as_mut_ptr().cast::<u8>();
        let aligned = base.add(base.align_offset(16)).cast::<AudioBufferList>();
        let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
        let status = sample.audio_buffer_list_with_retained_block_buffer(
            std::ptr::null_mut(),
            aligned,
            needed,
            None,
            None,
            AUDIO_BUFFER_LIST_ALIGNED,
            &mut block,
        );
        if status != 0 {
            return None;
        }
        let list = &*aligned;
        let buffers =
            std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize);
        let mut out = Vec::new();
        if planar {
            // One buffer a channel: interleave them.
            let planes: Vec<&[f32]> = buffers
                .iter()
                .filter(|b| !b.mData.is_null())
                .map(|b| {
                    std::slice::from_raw_parts(
                        b.mData.cast::<f32>().cast_const(),
                        b.mDataByteSize as usize / 4,
                    )
                })
                .collect();
            if let Some(frames) = planes.iter().map(|p| p.len()).min() {
                out.reserve_exact(frames * planes.len());
                for i in 0..frames {
                    for plane in &planes {
                        out.push(plane[i]);
                    }
                }
            }
        } else {
            for b in buffers.iter().filter(|b| !b.mData.is_null()) {
                out.extend_from_slice(std::slice::from_raw_parts(
                    b.mData.cast::<f32>().cast_const(),
                    b.mDataByteSize as usize / 4,
                ));
            }
            let _ = channels;
        }
        if !block.is_null() {
            drop(objc2_core_foundation::CFRetained::from_raw(
                std::ptr::NonNull::new_unchecked(block),
            ));
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The frame's pixels, packed: `None` for a sample with no picture (an idle
/// frame, when nothing on the display changed) or another pixel format.
pub(crate) fn copy_pixels(sample: &CMSampleBuffer) -> Option<(Vec<u8>, u32, u32)> {
    // SAFETY: the sample is the framework's for the duration of the call; its
    // pixel buffer is locked read-only while the rows are copied out and
    // unlocked after, and the copy stays within the buffer's rows.
    unsafe {
        let image = sample.image_buffer()?;
        if CVPixelBufferGetPixelFormatType(&image) != kCVPixelFormatType_32BGRA {
            return None;
        }
        if CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return None;
        }
        let width = CVPixelBufferGetWidth(&image);
        let height = CVPixelBufferGetHeight(&image);
        let stride = CVPixelBufferGetBytesPerRow(&image);
        let base = CVPixelBufferGetBaseAddress(&image)
            .cast::<u8>()
            .cast_const();
        let row = width * 4;
        let mut out = Vec::new();
        if !base.is_null() && stride >= row {
            out.reserve_exact(row * height);
            for y in 0..height {
                out.extend_from_slice(std::slice::from_raw_parts(base.add(y * stride), row));
            }
        }
        CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
        if out.is_empty() {
            return None;
        }
        Some((out, u32::try_from(width).ok()?, u32::try_from(height).ok()?))
    }
}

/// An object handed between threads by the completion handlers.
struct Handed<T>(T);
// SAFETY: the shareable content and errors are immutable once delivered and
// only read by the thread that waits for them.
unsafe impl<T> Send for Handed<T> {}

/// A running capture; dropping it stops the stream.
pub struct Stream {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stream")
    }
}

// SAFETY: an SCStream may be stopped from any thread; the output and queue
// are only kept alive here.
unsafe impl Send for Stream {}

impl Drop for Stream {
    fn drop(&mut self) {
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move |_error: *mut NSError| {
            let _ = tx.send(());
        });
        // SAFETY: stopping a started stream; the handler only signals.
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&done)) };
        let _ = rx.recv_timeout(ANSWER);
    }
}

fn error_text(error: *mut NSError) -> Option<(isize, String)> {
    // SAFETY: a non-null error from a completion handler is a live NSError
    // for the duration of the handler.
    let error = unsafe { error.as_ref() }?;
    Some((error.code(), error.localizedDescription().to_string()))
}

/// Captures `target` scaled to `width` by `height` at up to `rate` frames a
/// second, the cursor included, and with `audio` channels of the system's
/// sound to `on_audio` when both are given (macOS 13 and later,
/// [`audio_supported`]). A window is fitted into the picture with borders.
/// Blocks until ScreenCaptureKit has started the stream or refused.
///
/// # Errors
///
/// Returns why the stream did not start.
#[allow(clippy::too_many_arguments)]
pub fn stream(
    target: Target,
    width: u32,
    height: u32,
    rate: u32,
    audio: Option<u8>,
    on_frame: OnFrame,
    on_audio: Option<OnAudio>,
    on_end: OnEnd,
) -> Result<Stream, StartError> {
    if !screen_access() {
        return Err(StartError::Consent);
    }
    let content = shareable_content()?;

    // SAFETY: the content's displays and windows are live objects it owns;
    // the filter, configuration, output, queue and stream are made and wired
    // here as ScreenCaptureKit documents, and kept alive by the returned
    // `Stream`.
    unsafe {
        let filter = match target {
            Target::Display(id) => {
                let displays = content.displays();
                let Some(display) = displays.iter().find(|d| d.displayID() == id) else {
                    return Err(StartError::NoDisplay);
                };
                let none: Retained<NSArray<SCWindow>> = NSArray::new();
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &none,
                )
            }
            Target::Window(id) => {
                let windows = content.windows();
                let Some(window) = windows.iter().find(|w| w.windowID() == id) else {
                    return Err(StartError::NoDisplay);
                };
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
            }
        };
        let config = SCStreamConfiguration::new();
        config.setWidth(width as usize);
        config.setHeight(height as usize);
        config.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: i32::try_from(rate.max(1)).unwrap_or(30),
            flags: CMTimeFlags::Valid,
            epoch: 0,
        });
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setShowsCursor(true);
        config.setQueueDepth(5);
        if matches!(target, Target::Window(_)) {
            // The window's aspect kept, the spare area left blank.
            config.setScalesToFit(true);
        }
        let with_audio = audio.filter(|_| on_audio.is_some() && audio_supported());
        if let Some(channels) = with_audio {
            config.setCapturesAudio(true);
            config.setSampleRate(48_000);
            config.setChannelCount(isize::from(channels));
            config.setExcludesCurrentProcessAudio(true);
        }

        let output = Output::new(on_frame, on_audio.filter(|_| with_audio.is_some()), on_end);
        let delegate = ProtocolObject::<dyn SCStreamDelegate>::from_ref(&*output);
        let stream = SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(delegate),
        );
        let queue = DispatchQueue::new("org.ligatureprotocol.screen", None);
        let sink = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*output);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                sink,
                SCStreamOutputType::Screen,
                Some(&queue),
            )
            .map_err(|e| StartError::Failed(e.localizedDescription().to_string()))?;
        if with_audio.is_some() {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    sink,
                    SCStreamOutputType::Audio,
                    Some(&queue),
                )
                .map_err(|e| StartError::Failed(e.localizedDescription().to_string()))?;
        }

        let (tx, rx) = mpsc::channel::<Option<(isize, String)>>();
        let started = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(error_text(error));
        });
        stream.startCaptureWithCompletionHandler(Some(&started));
        match rx.recv_timeout(ANSWER) {
            Ok(None) => Ok(Stream {
                stream,
                _output: output,
                _queue: queue,
            }),
            Ok(Some((code, _))) if code == SCStreamErrorCode::UserDeclined.0 => {
                Err(StartError::Consent)
            }
            Ok(Some((_, text))) => Err(StartError::Failed(text)),
            Err(_) => Err(StartError::Failed("the stream did not start".to_owned())),
        }
    }
}
