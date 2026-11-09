//! AVFoundation's capture devices (milestone 10, LP-208): the Mac's cameras
//! listed, one captured as BGRA frames with the default microphone as float
//! samples, and the Camera and Microphone permissions.
//!
//! A capture is an `AVCaptureSession` with the camera's input and a video
//! data output, and the default microphone's input and an audio data output
//! when sound is wanted. Each output hands its sample buffers to a delegate
//! of its own on one serial queue: the two delegate protocols name the same
//! method, so one class cannot answer for both. The pictures come at the
//! size the session's default preset gives the camera and the samples in the
//! microphone's own format, both of which the caller is told, so nothing
//! here scales or resamples.
//!
//! macOS asks for the Camera and the Microphone separately, once each, in a
//! dialog of its own that names `Ligature.app`, whose `Info.plist` must say
//! why it wants each or macOS ends the process on first use. An answer
//! applies to the running process at once.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureAudioDataOutput, AVCaptureAudioDataOutputSampleBufferDelegate,
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureOutput, AVCaptureSession, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer,
    CMVideoFormatDescriptionGetDimensions,
};
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};

use crate::screen::{copy_pixels, copy_samples};

/// How long the person has to answer macOS's dialog when a share is what
/// asked for the permission.
const ASK: Duration = Duration::from_secs(30);

/// The highest frame rate a camera is listed with.
const MAX_RATE: u32 = 60;

/// `kCVPixelBufferPixelFormatTypeKey`, by its value: the key of the video
/// output's settings that names the pixel format.
const PIXEL_FORMAT_KEY: &str = "PixelFormatType";

/// What macOS has been told about a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Allowed.
    Allowed,
    /// Refused, or restricted by a policy.
    Denied,
    /// Not asked yet: the first use shows macOS's dialog.
    Unasked,
}

fn video() -> Option<&'static AVMediaType> {
    // SAFETY: a constant of the framework, read and never written.
    unsafe { AVMediaTypeVideo }
}

fn audio() -> Option<&'static AVMediaType> {
    // SAFETY: a constant of the framework, read and never written.
    unsafe { AVMediaTypeAudio }
}

fn access(media: Option<&'static AVMediaType>) -> Access {
    let Some(media) = media else {
        return Access::Denied;
    };
    // SAFETY: a query for one of the two media types it accepts.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    if status == AVAuthorizationStatus::Authorized {
        Access::Allowed
    } else if status == AVAuthorizationStatus::NotDetermined {
        Access::Unasked
    } else {
        Access::Denied
    }
}

/// Asks for `media`, which shows macOS's dialog when it was never asked,
/// and waits up to `wait` for the answer: whether it is allowed.
fn ask(media: Option<&'static AVMediaType>, wait: Duration) -> bool {
    let Some(media) = media else {
        return false;
    };
    let (tx, rx) = mpsc::channel::<bool>();
    let answered = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    // SAFETY: one of the two media types it accepts; the handler only
    // signals.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &answered) };
    rx.recv_timeout(wait).unwrap_or(false)
}

/// Whether the process may use a camera.
#[must_use]
pub fn camera_access() -> Access {
    access(video())
}

/// Whether the process may use the microphone.
#[must_use]
pub fn microphone_access() -> Access {
    access(audio())
}

/// Asks for the Camera, which shows macOS's dialog the first time and lists
/// Ligature under Camera in System Settings. Does not wait for the answer.
pub fn request_camera_access() {
    ask(video(), Duration::ZERO);
}

/// Asks for the Microphone, as [`request_camera_access`] does the Camera.
pub fn request_microphone_access() {
    ask(audio(), Duration::ZERO);
}

/// Where a camera is on the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// Facing the person at the screen.
    Front,
    /// Facing away.
    Back,
    /// Not part of the Mac, or not said.
    Unspecified,
}

/// One camera that can be shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    /// AVFoundation's unique id, which stays the same across reconnections.
    pub id: String,
    /// The name macOS shows.
    pub name: String,
    /// Where it is.
    pub position: Position,
    /// The width of its active format, in pixels.
    pub width: u32,
    /// The height of its active format, in pixels.
    pub height: u32,
    /// The frame rates its active format reaches, the highest first.
    pub rates: Vec<u32>,
}

/// The cameras connected now, the system's default first. Listing them
/// needs no permission.
#[must_use]
pub fn cameras() -> Vec<CameraDevice> {
    let Some(media) = video() else {
        return Vec::new();
    };
    // SAFETY: the devices are live objects the array owns; only their
    // read-only properties are read. `devicesWithMediaType:` is deprecated
    // for the discovery session, whose device types for external cameras are
    // constants that do not exist before macOS 14, where reading one is a
    // crash; the older call lists every camera on every version.
    unsafe {
        let preferred = AVCaptureDevice::defaultDeviceWithMediaType(media).map(|d| d.uniqueID());
        #[allow(deprecated)]
        let devices = AVCaptureDevice::devicesWithMediaType(media);
        let mut cameras: Vec<CameraDevice> = devices
            .iter()
            .filter(|d| d.isConnected() && !d.isSuspended())
            .map(|d| {
                let format = d.activeFormat();
                let size = CMVideoFormatDescriptionGetDimensions(&format.formatDescription());
                let mut rates: Vec<u32> = format
                    .videoSupportedFrameRateRanges()
                    .iter()
                    .map(|range| range.maxFrameRate().round())
                    .filter(|rate| *rate >= 1.0)
                    .map(|rate| (rate as u32).min(MAX_RATE))
                    .collect();
                rates.sort_unstable_by(|a, b| b.cmp(a));
                rates.dedup();
                let position = d.position();
                CameraDevice {
                    id: d.uniqueID().to_string(),
                    name: d.localizedName().to_string(),
                    position: if position == AVCaptureDevicePosition::Front {
                        Position::Front
                    } else if position == AVCaptureDevicePosition::Back {
                        Position::Back
                    } else {
                        Position::Unspecified
                    },
                    width: u32::try_from(size.width).unwrap_or(0),
                    height: u32::try_from(size.height).unwrap_or(0),
                    rates,
                }
            })
            .filter(|c| c.width > 0 && c.height > 0)
            .collect();
        if let Some(preferred) = preferred.map(|id| id.to_string()) {
            cameras.sort_by_key(|c| c.id != preferred);
        }
        cameras
    }
}

/// Why a capture did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// The Camera permission is not held.
    Consent,
    /// No connected camera has that id.
    NoCamera,
    /// AVFoundation refused, with its message.
    Failed(String),
}

/// One frame: BGRA pixels, rows packed with no padding, and its size.
pub type OnFrame = Box<dyn Fn(&[u8], u32, u32) + Send + Sync>;
/// One buffer of the microphone's sound: 32-bit float samples, the channels
/// interleaved, with their sample rate and how many channels there are.
pub type OnSound = Box<dyn Fn(&[f32], u32, u8) + Send + Sync>;

struct PictureIvars {
    on_frame: OnFrame,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "LigatureCameraPictures"]
    #[ivars = PictureIvars]
    struct Pictures;

    unsafe impl NSObjectProtocol for Pictures {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Pictures {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            if let Some((pixels, width, height)) = copy_pixels(sample) {
                (self.ivars().on_frame)(&pixels, width, height);
            }
        }
    }
);

struct SoundIvars {
    on_sound: OnSound,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "LigatureCameraSound"]
    #[ivars = SoundIvars]
    struct Sounds;

    unsafe impl NSObjectProtocol for Sounds {}

    unsafe impl AVCaptureAudioDataOutputSampleBufferDelegate for Sounds {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            if let (Some((rate, channels)), Some(samples)) =
                (sample_layout(sample), copy_samples(sample))
            {
                (self.ivars().on_sound)(&samples, rate, channels);
            }
        }
    }
);

/// The sample rate and channel count of a buffer of sound.
fn sample_layout(sample: &CMSampleBuffer) -> Option<(u32, u8)> {
    // SAFETY: the sample is the framework's for the duration of the call,
    // and the description it points to lives as long as the sample.
    unsafe {
        let description = sample.format_description()?;
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&description);
        if asbd.is_null() {
            return None;
        }
        let asbd = *asbd;
        let rate = asbd.mSampleRate.round();
        let channels = u8::try_from(asbd.mChannelsPerFrame).ok()?;
        ((8000.0..=384_000.0).contains(&rate) && channels > 0).then_some((rate as u32, channels))
    }
}

/// A running capture; dropping it stops the session, which turns the
/// camera's light off.
pub struct CameraStream {
    session: Retained<AVCaptureSession>,
    video: Retained<AVCaptureVideoDataOutput>,
    sound: Option<Retained<AVCaptureAudioDataOutput>>,
    _pictures: Retained<Pictures>,
    _sounds: Option<Retained<Sounds>>,
    _queue: DispatchRetained<DispatchQueue>,
    /// Whether the microphone is captured with the camera.
    pub microphone: bool,
}

impl std::fmt::Debug for CameraStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CameraStream")
    }
}

// SAFETY: an AVCaptureSession may be stopped from any thread; the outputs,
// delegates and queue are only kept alive here and released with it.
unsafe impl Send for CameraStream {}

impl Drop for CameraStream {
    fn drop(&mut self) {
        // SAFETY: stopping a started session and detaching the delegates the
        // outputs do not retain, before they are released.
        unsafe {
            self.session.stopRunning();
            self.video.setSampleBufferDelegate_queue(None, None);
            if let Some(sound) = &self.sound {
                sound.setSampleBufferDelegate_queue(None, None);
            }
        }
    }
}

/// Captures the camera `id`, its pictures to `on_frame` as BGRA at the size
/// and rate the camera gives, and, when `on_sound` is given and the
/// Microphone is allowed, the default microphone's samples to it. Asks for
/// each permission the first time, waiting for the answer, and blocks until
/// the session runs. A microphone that is refused or missing leaves the
/// camera silent rather than refused ([`CameraStream::microphone`] says).
///
/// # Errors
///
/// Returns why the capture did not start.
pub fn camera_stream(
    id: &str,
    on_frame: OnFrame,
    on_sound: Option<OnSound>,
) -> Result<CameraStream, StartError> {
    match camera_access() {
        Access::Allowed => {}
        Access::Unasked if ask(video(), ASK) => {}
        Access::Unasked | Access::Denied => return Err(StartError::Consent),
    }
    let with_sound = on_sound.is_some()
        && match microphone_access() {
            Access::Allowed => true,
            Access::Unasked => ask(audio(), ASK),
            Access::Denied => false,
        };

    // SAFETY: the device, inputs, outputs, delegates, queue and session are
    // made and wired here as AVFoundation documents, on one thread, and kept
    // alive by the returned `CameraStream`.
    unsafe {
        let Some(device) = AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(id))
            .filter(|d| d.isConnected())
        else {
            return Err(StartError::NoCamera);
        };
        let failed = |e: Retained<objc2_foundation::NSError>| {
            StartError::Failed(e.localizedDescription().to_string())
        };
        let session = AVCaptureSession::new();
        session.beginConfiguration();
        let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device).map_err(failed)?;
        if !session.canAddInput(&input) {
            return Err(StartError::Failed(
                "the camera cannot be added to a capture session".to_owned(),
            ));
        }
        session.addInput(&input);

        let queue = DispatchQueue::new("org.ligatureprotocol.camera", None);
        let video = AVCaptureVideoDataOutput::new();
        let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
        let format: &AnyObject = &format;
        let settings: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[&*NSString::from_str(PIXEL_FORMAT_KEY)], &[format]);
        video.setVideoSettings(Some(&settings));
        // A picture the queue has no time for is dropped, never queued.
        video.setAlwaysDiscardsLateVideoFrames(true);
        let pictures: Retained<Pictures> = {
            let this = Pictures::alloc().set_ivars(PictureIvars { on_frame });
            msg_send![super(this), init]
        };
        video.setSampleBufferDelegate_queue(
            Some(ProtocolObject::<
                dyn AVCaptureVideoDataOutputSampleBufferDelegate,
            >::from_ref(&*pictures)),
            Some(&queue),
        );
        if !session.canAddOutput(&video) {
            return Err(StartError::Failed(
                "the camera's pictures cannot be taken from a capture session".to_owned(),
            ));
        }
        session.addOutput(&video);

        // The microphone: the system's default input, in its own format.
        let mut sound = None;
        let mut sounds = None;
        if let (true, Some(on_sound), Some(media)) = (with_sound, on_sound, audio()) {
            let microphone = AVCaptureDevice::defaultDeviceWithMediaType(media)
                .and_then(|d| AVCaptureDeviceInput::deviceInputWithDevice_error(&d).ok())
                .filter(|input| session.canAddInput(input));
            let output = AVCaptureAudioDataOutput::new();
            if let Some(microphone) = microphone.filter(|_| session.canAddOutput(&output)) {
                session.addInput(&microphone);
                let delegate: Retained<Sounds> = {
                    let this = Sounds::alloc().set_ivars(SoundIvars { on_sound });
                    msg_send![super(this), init]
                };
                output.setSampleBufferDelegate_queue(
                    Some(ProtocolObject::<
                        dyn AVCaptureAudioDataOutputSampleBufferDelegate,
                    >::from_ref(&*delegate)),
                    Some(&queue),
                );
                session.addOutput(&output);
                sound = Some(output);
                sounds = Some(delegate);
            }
        }
        session.commitConfiguration();
        session.startRunning();
        if !session.isRunning() {
            return Err(StartError::Failed(
                "the capture session did not start".to_owned(),
            ));
        }
        Ok(CameraStream {
            microphone: sound.is_some(),
            session,
            video,
            sound,
            _pictures: pictures,
            _sounds: sounds,
            _queue: queue,
        })
    }
}
