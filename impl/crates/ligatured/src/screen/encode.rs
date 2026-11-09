//! H.264 encoding by ffmpeg, the same on every desktop (milestone 8): the
//! encoder this machine has, the arguments that turn raw frames on stdin into
//! an Annex B stream on stdout with a delimiter before every frame and the
//! parameter sets before every keyframe, the split of that stream into
//! frames, and the [`LiveEncoder`] that feeds it and starts it again for a
//! keyframe. Linux feeds it from the portal or the screencopy helper
//! (LP-149), the Mac from ScreenCaptureKit (LP-150).

use std::io::{Read, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lp_core::screen::{Chroma, Format, RefuseReason, StopReason};
use tokio::sync::mpsc;

use crate::screen::{Captured, EncodedFrame, Settings};

/// The picture formats an encoder produces, most preferred first (ADR
/// 0044): x264 encodes 4:4:4, which keeps coloured text sharp, and 4:2:0;
/// the hardware encoders 4:2:0 alone.
#[must_use]
pub fn encodes(encoder: Encoder) -> Vec<Format> {
    match encoder {
        Encoder::X264 => vec![Format::H264_444, Format::H264],
        Encoder::Vaapi | Encoder::VideoToolbox => vec![Format::H264],
    }
}

/// Bits per pixel per frame a hardware encoder aims for.
const BITS_PER_PIXEL: f64 = 0.08;
/// The most a hardware encoder may send, as a multiple of [`bit_rate`],
/// over a buffer of one second: room for a sharp keyframe of a screen of
/// text.
const CAP_FACTOR: u64 = 4;
/// x264's constant quantiser. Held constant, a part of the screen that did
/// not change is copied from the frame before and never coded again, so
/// settled text keeps the same pixels; a quality held by rate control
/// instead moves the quantiser from frame to frame and redraws the text with
/// new artifacts.
pub const X264_QP: &str = "18";
/// The constant rate factor x264 codes a camera's picture at (milestone 10
/// decision 3): its default, good for a face and a room, under the rate cap.
pub const X264_CAMERA_CRF: &str = "23";
/// The most frames between keyframes, as a safety net: keyframes come at
/// the start of an encoder and when one is asked for ([`LiveEncoder`]).
pub const KEYFRAME_BACKSTOP: u32 = 10_000;
/// The least time after a new picture before the encoder is given it again
/// when nothing newer came, and never under one and a half frames of the
/// share's rate, so the copy has a timestamp of its own. The daemon knows a
/// frame has ended only when the next one begins, and ffmpeg's decoder in
/// `lpview` holds a picture back until the next one too, so the last
/// picture of a change would otherwise wait for the next change or the
/// repeat a second later.
pub const SETTLE: Duration = Duration::from_millis(50);

/// How long [`LiveEncoder`] waits to settle a picture at `rate` frames a
/// second.
#[must_use]
pub fn settle_after(rate: u32) -> Duration {
    (Duration::from_secs(1) * 3 / (2 * rate.max(1))).max(SETTLE)
}

/// The H.264 encoder ffmpeg offers here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    /// VA-API on the render node.
    Vaapi,
    /// `libx264` in software.
    X264,
    /// VideoToolbox on a Mac.
    VideoToolbox,
}

/// The encoder's bit rate for a picture and rate, in bits per second.
#[must_use]
pub fn bit_rate(width: u32, height: u32, rate: u32) -> u64 {
    let bits = f64::from(width) * f64::from(height) * f64::from(rate) * BITS_PER_PIXEL;
    (bits as u64).clamp(1_000_000, 20_000_000)
}

/// Where the raw frames come from: their size and ffmpeg's name for their
/// pixel format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawInput {
    /// Width of each frame.
    pub width: u32,
    /// Height of each frame.
    pub height: u32,
    /// ffmpeg's `pix_fmt`: `bgr0`, `rgb0`, `bgra` or `rgba`.
    pub pix_fmt: String,
}

/// The arguments of the ffmpeg that encodes raw frames on stdin into an
/// H.264 byte stream on stdout, scaled to the share's size. Frames come as
/// the screen changes, at most at the share's rate: each is stamped with the
/// wall time it arrived and encoded once, never repeated to fill the rate.
/// The first frame is a keyframe, and the next only after
/// [`KEYFRAME_BACKSTOP`] frames, since a new encoder makes one when it is
/// asked for.
#[must_use]
pub fn encoder_args(encoder: Encoder, input: &RawInput, settings: &Settings) -> Vec<String> {
    let rate = settings.frame_rate.max(1);
    let rate_s = rate.to_string();
    let br = bit_rate(settings.width, settings.height, rate);
    let camera = settings.kind == lp_core::screen::SourceKind::Camera;
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if encoder == Encoder::Vaapi {
        args.extend(["-vaapi_device", "/dev/dri/renderD128"].map(str::to_owned));
    }
    args.extend(
        [
            "-f",
            "rawvideo",
            "-pix_fmt",
            &input.pix_fmt,
            "-video_size",
            &format!("{}x{}", input.width, input.height),
            "-framerate",
            &rate_s,
            "-use_wallclock_as_timestamps",
            "1",
            "-i",
            "pipe:0",
        ]
        .map(str::to_owned),
    );
    // A screen is captured at the share's size already. A camera's pictures
    // come at the size the platform gives it, so their shape is kept inside
    // the agreed picture, the spare area black.
    let (w, h) = (settings.width, settings.height);
    let scale = if camera {
        format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
        )
    } else {
        format!("scale={w}:{h}")
    };
    match encoder {
        Encoder::Vaapi => args.extend(
            [
                "-vf",
                &format!("{scale},format=nv12,hwupload"),
                "-c:v",
                "h264_vaapi",
                // One frame in flight, not the default two: it held each
                // frame 39 ms here (LP-176).
                "-async_depth",
                "1",
                "-aud",
                "1",
                "-bsf:v",
                "dump_extra",
            ]
            .map(str::to_owned),
        ),
        Encoder::VideoToolbox => args.extend(
            [
                "-vf",
                &format!("{scale},format=nv12"),
                "-c:v",
                "h264_videotoolbox",
                "-realtime",
                "1",
                "-prio_speed",
                "1",
                // VideoToolbox writes no delimiters and keeps the parameter
                // sets in the stream's header: the parameter sets go before
                // every keyframe, then a delimiter before every frame.
                "-bsf:v",
                "dump_extra=freq=keyframe,h264_metadata=aud=insert",
            ]
            .map(str::to_owned),
        ),
        Encoder::X264 => args.extend(
            [
                "-vf",
                &scale,
                "-c:v",
                "libx264",
                "-preset",
                "superfast",
                "-tune",
                "zerolatency",
                // A screen holds its quantiser, so what did not change is
                // copied; a camera's picture changes everywhere all the
                // time, so it is coded to a quality under a rate cap.
                if camera { "-crf" } else { "-qp" },
                if camera { X264_CAMERA_CRF } else { X264_QP },
                "-pix_fmt",
                match settings.format.chroma {
                    Chroma::Yuv420 => "yuv420p",
                    // High 4:4:4 Predictive, chosen by x264 for this input.
                    Chroma::Yuv444 => "yuv444p",
                },
                "-x264-params",
                "aud=1:repeat-headers=1",
            ]
            .map(str::to_owned),
        ),
    }
    // The hardware encoders aim for a rate under a cap; x264 holds its
    // quantiser for a screen instead, which a cap would move, and takes the
    // cap alone for a camera.
    let cap = (br * CAP_FACTOR).to_string();
    if encoder != Encoder::X264 {
        args.extend(
            ["-b:v", &br.to_string(), "-maxrate", &cap, "-bufsize", &cap].map(str::to_owned),
        );
    } else if camera {
        args.extend(["-maxrate", &cap, "-bufsize", &cap].map(str::to_owned));
    }
    let gop = KEYFRAME_BACKSTOP.to_string();
    args.extend(
        [
            "-g",
            &gop,
            "-bf",
            "0",
            "-fps_mode",
            "passthrough",
            "-flush_packets",
            "1",
            "-f",
            "h264",
            "pipe:1",
        ]
        .map(str::to_owned),
    );
    args
}

/// Splits an H.264 Annex B byte stream into access units at the access unit
/// delimiters the encoder puts before every frame, and, through
/// [`AccessUnits::take_quiet`], cuts a frame that looks whole when the
/// encoder pauses, rather than holding it until the next frame begins
/// (LP-176).
#[derive(Debug, Default)]
pub struct AccessUnits {
    pending: Vec<u8>,
    /// A frame was cut on a pause, and the next bytes must start a frame.
    after_cut: bool,
    /// Frames found cut short: bytes after a cut that did not start a frame.
    cut_short: u64,
}

/// How long the encoder's stream must be quiet before a whole-looking frame is
/// cut: ffmpeg writes a packet with back-to-back writes (`-flush_packets 1`),
/// so a frame's own gaps are microseconds.
pub const QUIET: Duration = Duration::from_millis(3);

/// The NAL unit type of an access unit delimiter.
const NAL_AUD: u8 = 9;
/// The NAL unit type of an IDR slice.
const NAL_IDR: u8 = 5;

/// Every NAL unit's start (the index of its first header byte) and type.
fn nal_units(bytes: &[u8]) -> Vec<(usize, usize, u8)> {
    // (start code index, header index, type)
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1 {
            let code = if i > 0 && bytes[i - 1] == 0 { i - 1 } else { i };
            out.push((code, i + 3, bytes[i + 3] & 0x1f));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// Whether bytes begin with an access unit delimiter.
fn starts_with_delimiter(bytes: &[u8]) -> bool {
    nal_units(bytes)
        .first()
        .is_some_and(|(code, _, t)| *code == 0 && *t == NAL_AUD)
}

/// Whether an access unit holds an IDR slice, so a decoder can start there.
#[must_use]
pub fn is_keyframe(unit: &[u8]) -> bool {
    nal_units(unit).iter().any(|(_, _, t)| *t == NAL_IDR)
}

impl AccessUnits {
    /// Takes more bytes and returns every access unit they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        if self.after_cut && self.pending.len() >= 5 {
            self.after_cut = false;
            if !starts_with_delimiter(&self.pending) {
                self.cut_short += 1;
                tracing::warn!("a frame was cut short on a pause in the encoder's stream");
            }
        }
        let starts: Vec<usize> = nal_units(&self.pending)
            .into_iter()
            .filter(|(_, _, t)| *t == NAL_AUD)
            .map(|(code, _, _)| code)
            .collect();
        let mut out = Vec::new();
        let mut cut = 0;
        for start in starts {
            if start > cut {
                out.push(self.pending[cut..start].to_vec());
            }
            cut = start;
        }
        // An access unit is complete only when the next one's delimiter has
        // arrived; what follows the last delimiter waits.
        self.pending.drain(..cut);
        out.retain(|u| !u.is_empty());
        out
    }

    /// The frame waiting for the next delimiter, if it looks whole: it
    /// begins with a delimiter and holds a slice. Called when the encoder's
    /// stream has been quiet for [`QUIET`].
    pub fn take_quiet(&mut self) -> Option<Vec<u8>> {
        if !starts_with_delimiter(&self.pending) {
            return None;
        }
        let has_slice = nal_units(&self.pending)
            .iter()
            .any(|(_, header, t)| (1..=5).contains(t) && *header + 1 < self.pending.len());
        if !has_slice {
            return None;
        }
        self.after_cut = true;
        Some(std::mem::take(&mut self.pending))
    }

    /// How many frames were found cut short by [`AccessUnits::take_quiet`].
    #[must_use]
    pub fn cut_short(&self) -> u64 {
        self.cut_short
    }

    /// What is left when the stream ends.
    pub fn finish(&mut self) -> Option<Vec<u8>> {
        let rest = std::mem::take(&mut self.pending);
        (!rest.is_empty()).then_some(rest)
    }
}

fn run_quiet(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Which H.264 encoder ffmpeg has that works here: VideoToolbox on a Mac or
/// VA-API on the render node if a test encode succeeds, else `libx264`, else
/// none.
#[must_use]
pub fn probe_encoder() -> Option<Encoder> {
    let encoders = String::from_utf8(run_quiet("ffmpeg", &["-hide_banner", "-encoders"])?).ok()?;
    if encoders.contains(" h264_videotoolbox ") {
        let test = run_quiet(
            "ffmpeg",
            &[
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=5",
                "-frames:v",
                "2",
                "-vf",
                "format=nv12",
                "-c:v",
                "h264_videotoolbox",
                "-f",
                "h264",
                "pipe:1",
            ],
        );
        if test.is_some_and(|b| !b.is_empty()) {
            return Some(Encoder::VideoToolbox);
        }
    }
    if encoders.contains(" h264_vaapi ") && Path::new("/dev/dri/renderD128").exists() {
        let test = run_quiet(
            "ffmpeg",
            &[
                "-hide_banner",
                "-loglevel",
                "error",
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=5",
                "-frames:v",
                "2",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
                "-f",
                "h264",
                "pipe:1",
            ],
        );
        if test.is_some_and(|b| !b.is_empty()) {
            return Some(Encoder::Vaapi);
        }
    }
    encoders.contains(" libx264 ").then_some(Encoder::X264)
}

/// Reads an encoder's stdout until it ends, sending each access unit as a
/// frame; when the stream ends without `stopping` set, the share ends with
/// an error. Runs on a thread of its own.
///
/// # Errors
///
/// Returns the error when the thread cannot be started.
pub fn read_frames(
    encoded: impl Read + Send + 'static,
    frames: mpsc::UnboundedSender<Captured>,
    stopping: Arc<AtomicBool>,
) -> std::io::Result<()> {
    // One thread reads; the other cuts, on a delimiter or on a pause, since a
    // blocked read cannot time out.
    let (chunks, chunks_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("screen-encoded".into())
        .spawn(move || {
            let mut encoded = encoded;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                match encoded.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if chunks.send(buf[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        })?;
    std::thread::Builder::new()
        .name("screen-framer".into())
        .spawn(move || {
            let mut units = AccessUnits::default();
            loop {
                let ready = match chunks_rx.recv_timeout(QUIET) {
                    Ok(chunk) => units.push(&chunk),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        units.take_quiet().into_iter().collect()
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                for unit in ready {
                    let key = is_keyframe(&unit);
                    let frame = EncodedFrame {
                        at: 0,
                        key,
                        bytes: unit,
                    };
                    if frames.send(Captured::Frame(frame)).is_err() {
                        return;
                    }
                }
            }
            if !stopping.load(Ordering::SeqCst) {
                tracing::info!("the encoder's stream ended");
                let _ = frames.send(Captured::Ended(StopReason::Error));
            }
        })
        .map(|_| ())
}

/// A reader that notes when it last read anything, for a share's watchdog.
pub struct Noting<R> {
    inner: R,
    last: Arc<Mutex<Instant>>,
}

impl<R> Noting<R> {
    /// Notes the reads of `inner` in `last`.
    pub fn new(inner: R, last: Arc<Mutex<Instant>>) -> Self {
        Self { inner, last }
    }
}

impl<R: Read> Read for Noting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            *lock(&self.last) = Instant::now();
        }
        Ok(n)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What goes into the running encoder.
struct Feed {
    stdin: Option<ChildStdin>,
    input: RawInput,
    /// The last picture written, given again to settle it and to a new
    /// encoder.
    last: Vec<u8>,
    /// Counts pictures that differ from the one before.
    changed: u64,
    /// The count when the last picture was settled.
    settled: u64,
    /// A keyframe was asked for and the encoder is not yet started again.
    want_key: bool,
}

/// The running ffmpeg, and the flag that keeps its end from being taken for
/// a failure once it is replaced or stopped.
struct Process {
    child: Option<Child>,
    generation: Arc<AtomicBool>,
}

/// A share's ffmpeg, fed one raw picture at a time. ffmpeg cannot be asked
/// for a keyframe once it runs, and a new encoder begins with one, so a
/// keyframe request starts it again on the last picture; requests that come
/// while it starts are answered by that keyframe. A thread of its own does
/// the restarts and gives the encoder the last picture of a change again
/// after [`settle_after`], so that picture is not held back at either end.
pub struct LiveEncoder {
    encoder: Encoder,
    settings: Settings,
    frames: mpsc::UnboundedSender<Captured>,
    output: Arc<Mutex<Instant>>,
    stopped: AtomicBool,
    feed: Mutex<Feed>,
    fed: Condvar,
    process: Mutex<Process>,
}

impl std::fmt::Debug for LiveEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveEncoder")
    }
}

impl LiveEncoder {
    /// Starts the encoder for pictures of `input`, its frames sent to
    /// `frames`, and notes in `output` when it last wrote anything.
    ///
    /// # Errors
    ///
    /// `NoEncoder` when ffmpeg or the threads cannot be started.
    pub fn start(
        encoder: Encoder,
        input: RawInput,
        settings: &Settings,
        frames: mpsc::UnboundedSender<Captured>,
        output: Arc<Mutex<Instant>>,
    ) -> Result<Arc<Self>, RefuseReason> {
        let this = Arc::new(Self {
            encoder,
            settings: settings.clone(),
            frames,
            output,
            stopped: AtomicBool::new(false),
            feed: Mutex::new(Feed {
                stdin: None,
                input,
                last: Vec::new(),
                changed: 0,
                settled: 0,
                want_key: false,
            }),
            fed: Condvar::new(),
            process: Mutex::new(Process {
                child: None,
                generation: Arc::new(AtomicBool::new(true)),
            }),
        });
        {
            let mut feed = lock(&this.feed);
            let input = feed.input.clone();
            feed.stdin = Some(this.spawn(&input)?);
        }
        let worker = this.clone();
        std::thread::Builder::new()
            .name("screen-feeder".into())
            .spawn(move || worker.work())
            .map_err(|e| {
                tracing::warn!(%e, "cannot start the screen feeder");
                this.stop();
                RefuseReason::NoEncoder
            })?;
        Ok(this)
    }

    /// Starts an ffmpeg for `input` in place of the running one, which is
    /// let go first, so the end of its stream is not taken for a failure;
    /// returns its stdin.
    fn spawn(&self, input: &RawInput) -> Result<ChildStdin, RefuseReason> {
        let mut process = lock(&self.process);
        if self.stopped.load(Ordering::SeqCst) {
            return Err(RefuseReason::NoEncoder);
        }
        process.generation.store(true, Ordering::SeqCst);
        if let Some(mut old) = process.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        let mut child = Command::new("ffmpeg")
            .args(encoder_args(self.encoder, input, &self.settings))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                tracing::warn!(%e, "cannot start ffmpeg");
                RefuseReason::NoEncoder
            })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RefuseReason::NoEncoder);
        };
        let generation = Arc::new(AtomicBool::new(false));
        if let Err(e) = read_frames(
            Noting::new(stdout, self.output.clone()),
            self.frames.clone(),
            generation.clone(),
        ) {
            tracing::warn!(%e, "cannot read the encoder");
            let _ = child.kill();
            let _ = child.wait();
            return Err(RefuseReason::NoEncoder);
        }
        process.child = Some(child);
        process.generation = generation;
        Ok(stdin)
    }

    /// Writes a picture of the input's size into the encoder.
    ///
    /// # Errors
    ///
    /// The write's error, once the encoder is gone.
    pub fn write(&self, picture: &[u8]) -> std::io::Result<()> {
        let mut feed = lock(&self.feed);
        let stdin = feed
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        stdin.write_all(picture)?;
        if feed.last != picture {
            feed.last.clear();
            feed.last.extend_from_slice(picture);
            feed.changed += 1;
            self.fed.notify_all();
        }
        Ok(())
    }

    /// Asks for a keyframe: the encoder is started again on the last picture
    /// by the feeder thread.
    pub fn keyframe(&self) {
        lock(&self.feed).want_key = true;
        self.fed.notify_all();
    }

    /// Starts the encoder again for pictures of `input`, with a keyframe;
    /// the next pictures come in through [`Self::write`]. Blocks while
    /// ffmpeg starts.
    ///
    /// # Errors
    ///
    /// `NoEncoder` when ffmpeg cannot be started.
    pub fn restart(&self, input: RawInput) -> Result<(), RefuseReason> {
        let mut feed = lock(&self.feed);
        if feed.input != input {
            feed.last.clear();
        }
        feed.stdin = Some(self.spawn(&input)?);
        feed.input = input;
        Ok(())
    }

    /// Stops the encoder for good; its end is not taken for a failure.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.fed.notify_all();
        let mut process = lock(&self.process);
        process.generation.store(true, Ordering::SeqCst);
        if let Some(mut child) = process.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The feeder thread: restarts for keyframes and settles the last
    /// picture of a change.
    fn work(&self) {
        let mut feed = lock(&self.feed);
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            if feed.want_key {
                feed.want_key = false;
                // A new encoder whose first picture is the last one: a
                // keyframe of the screen as it is, then settled as any.
                let input = feed.input.clone();
                match self.spawn(&input) {
                    Ok(mut stdin) => {
                        tracing::debug!("encoder started again for a keyframe");
                        if !feed.last.is_empty() && stdin.write_all(&feed.last).is_ok() {
                            feed.changed += 1;
                        }
                        feed.stdin = Some(stdin);
                    }
                    Err(_) => {
                        if !self.stopped.load(Ordering::SeqCst) {
                            let _ = self.frames.send(Captured::Ended(StopReason::Error));
                        }
                        return;
                    }
                }
                continue;
            }
            if feed.changed != feed.settled {
                let seen = feed.changed;
                feed = self
                    .fed
                    .wait_timeout_while(feed, settle_after(self.settings.frame_rate), |f| {
                        f.changed == seen && !f.want_key && !self.stopped.load(Ordering::SeqCst)
                    })
                    .map_or_else(|e| e.into_inner().0, |(g, _)| g);
                if feed.changed == seen && !feed.want_key && !self.stopped.load(Ordering::SeqCst) {
                    let Feed { stdin, last, .. } = &mut *feed;
                    if let Some(stdin) = stdin.as_mut() {
                        let _ = stdin.write_all(last);
                    }
                    feed.settled = seen;
                }
                continue;
            }
            feed = self
                .fed
                .wait_while(feed, |f| {
                    f.changed == f.settled && !f.want_key && !self.stopped.load(Ordering::SeqCst)
                })
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Milestone 10 decision 3: a camera's picture is coded to a quality
    /// under a rate cap, a screen's at a constant quantiser with none.
    #[test]
    fn a_camera_is_coded_to_a_rate_and_a_screen_to_a_quantiser() {
        let input = RawInput {
            width: 1280,
            height: 720,
            pix_fmt: "bgr0".to_owned(),
        };
        let settings = |kind| Settings {
            display: "0".to_owned(),
            kind,
            format: Format::H264,
            audio: None,
            width: 1280,
            height: 720,
            frame_rate: 30,
            viewer: String::new(),
            cast: false,
        };
        let camera = encoder_args(
            Encoder::X264,
            &input,
            &settings(lp_core::screen::SourceKind::Camera),
        )
        .join(" ");
        assert!(camera.contains("-crf 23"), "{camera}");
        assert!(!camera.contains("-qp "), "{camera}");
        let cap = bit_rate(1280, 720, 30) * CAP_FACTOR;
        assert!(
            camera.contains(&format!("-maxrate {cap} -bufsize {cap}")),
            "{camera}"
        );
        assert!(
            !camera.contains("-b:v"),
            "x264 takes the cap alone: {camera}"
        );
        // A camera's shape is kept within the picture; a screen comes at the
        // picture's size.
        assert!(
            camera.contains(
                "-vf scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2 -c:v"
            ),
            "{camera}"
        );
        let screen = encoder_args(
            Encoder::X264,
            &input,
            &settings(lp_core::screen::SourceKind::Display),
        )
        .join(" ");
        assert!(
            screen.contains("-qp 18") && !screen.contains("-crf"),
            "{screen}"
        );
        assert!(!screen.contains("-maxrate"), "{screen}");
        assert!(screen.contains("-vf scale=1280:720 -c:v"), "{screen}");
        // The hardware encoders aim at a rate either way.
        let hardware = encoder_args(
            Encoder::Vaapi,
            &input,
            &settings(lp_core::screen::SourceKind::Camera),
        )
        .join(" ");
        assert!(
            hardware.contains("-b:v") && hardware.contains("-maxrate"),
            "{hardware}"
        );
    }

    const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xf0];
    const SPS: [u8; 7] = [0, 0, 0, 1, 0x67, 1, 2];
    const IDR: [u8; 7] = [0, 0, 1, 0x65, 9, 9, 9];
    const P: [u8; 7] = [0, 0, 0, 1, 0x41, 7, 7];

    #[test]
    fn a_frame_that_looks_whole_is_cut_when_the_stream_goes_quiet() {
        // The next frame's delimiter may be a frame interval away, or longer
        // on a still screen: a whole-looking frame goes when the encoder
        // pauses (LP-176).
        let mut units = AccessUnits::default();
        let frame = [AUD.as_slice(), &SPS, &IDR].concat();
        assert!(units.push(&frame).is_empty());
        assert_eq!(units.take_quiet(), Some(frame.clone()));
        assert_eq!(units.take_quiet(), None);
        // The next frame comes whole through the delimiter rule as before.
        let next = [AUD.as_slice(), &P].concat();
        assert!(units.push(&next).is_empty());
        assert_eq!(units.push(&AUD), vec![next]);
    }

    #[test]
    fn a_frame_without_a_slice_yet_is_not_cut() {
        let mut units = AccessUnits::default();
        assert!(units.push(&[AUD.as_slice(), &SPS].concat()).is_empty());
        assert_eq!(units.take_quiet(), None);
        // A frame not starting with a delimiter is not taken for whole.
        let mut units = AccessUnits::default();
        assert!(units.push(&P).is_empty());
        assert_eq!(units.take_quiet(), None);
    }

    #[test]
    fn bytes_after_a_cut_that_do_not_start_a_frame_are_noted() {
        let mut units = AccessUnits::default();
        units.push(&[AUD.as_slice(), &P].concat());
        assert!(units.take_quiet().is_some());
        // The rest of a frame cut short: it still goes out, at the next
        // delimiter, and the cut is counted.
        units.push(&[9, 9, 9, 9, 9, 9]);
        assert_eq!(units.cut_short(), 1);
        let out = units.push(&AUD);
        assert_eq!(out, vec![vec![9, 9, 9, 9, 9, 9]]);
    }

    #[test]
    fn vaapi_does_not_pipeline_frames() {
        let input = RawInput {
            width: 1280,
            height: 720,
            pix_fmt: "bgr0".to_owned(),
        };
        let settings = Settings {
            display: "1".to_owned(),
            kind: lp_core::screen::SourceKind::Display,
            format: Format::H264,
            audio: None,
            width: 1280,
            height: 720,
            frame_rate: 30,
            viewer: String::new(),
            cast: false,
        };
        let args = encoder_args(Encoder::Vaapi, &input, &settings).join(" ");
        assert!(args.contains("-c:v h264_vaapi -async_depth 1"), "{args}");
    }

    #[test]
    fn videotoolbox_gets_delimiters_and_parameter_sets_from_bitstream_filters() {
        let input = RawInput {
            width: 2880,
            height: 1800,
            pix_fmt: "bgra".to_owned(),
        };
        let settings = Settings {
            display: "1".to_owned(),
            kind: lp_core::screen::SourceKind::Display,
            format: Format::H264,
            audio: None,
            width: 1440,
            height: 900,
            frame_rate: 30,
            viewer: "linux".to_owned(),
            cast: false,
        };
        let args = encoder_args(Encoder::VideoToolbox, &input, &settings).join(" ");
        assert!(args.contains(
            "-pix_fmt bgra -video_size 2880x1800 -framerate 30 \
             -use_wallclock_as_timestamps 1 -i pipe:0"
        ));
        assert!(args.contains("-vf scale=1440:900,format=nv12 -c:v h264_videotoolbox"));
        assert!(args.contains("-bsf:v dump_extra=freq=keyframe,h264_metadata=aud=insert"));
        assert!(args.contains(&format!("-g {KEYFRAME_BACKSTOP}")));
        assert!(args.contains("-b:v 3110400 -maxrate 12441600 -bufsize 12441600"));
        assert!(args.contains("-bf 0"));
        assert!(args.ends_with("-f h264 pipe:1"));
    }

    /// A share picked at 4:4:4 comes out of x264 as a full-colour stream
    /// that decodes (ADR 0044).
    #[test]
    fn a_4_4_4_share_is_a_full_colour_stream() {
        use std::io::Write as _;
        if probe_encoder().is_none() {
            eprintln!("skipped: ffmpeg has no H.264 encoder here");
            return;
        }
        let (w, h) = (64u32, 48u32);
        let input = RawInput {
            width: w,
            height: h,
            pix_fmt: "bgr0".to_owned(),
        };
        let settings = Settings {
            display: "1".to_owned(),
            kind: lp_core::screen::SourceKind::Display,
            format: Format::H264_444,
            audio: None,
            width: w,
            height: h,
            frame_rate: 30,
            viewer: "test".to_owned(),
            cast: false,
        };
        let mut child = Command::new("ffmpeg")
            .args(encoder_args(Encoder::X264, &input, &settings))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("ffmpeg runs");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            out
        });
        for shade in [10u8, 120, 240] {
            stdin.write_all(&vec![shade; (w * h * 4) as usize]).unwrap();
            std::thread::sleep(Duration::from_millis(40));
        }
        drop(stdin);
        let stream = reader.join().unwrap();
        let _ = child.wait();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.h264");
        std::fs::write(&path, &stream).unwrap();
        let probe = Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
            .args([
                "-show_entries",
                "stream=pix_fmt,profile,nb_read_frames",
                "-of",
                "csv=p=0",
            ])
            .arg(&path)
            .output()
            .expect("ffprobe runs");
        let probed = String::from_utf8_lossy(&probe.stdout).trim().to_owned();
        assert!(probed.contains("High 4:4:4"), "{probed}");
        assert!(probed.contains("yuv444p"), "{probed}");
        assert!(probed.ends_with(",3"), "every frame decodes: {probed}");
    }

    #[test]
    fn a_picture_settles_after_a_frame_and_a_half_and_never_sooner_than_50_ms() {
        assert_eq!(settle_after(30), Duration::from_millis(50));
        assert_eq!(settle_after(60), Duration::from_millis(50));
        assert_eq!(settle_after(15), Duration::from_millis(100));
        assert_eq!(settle_after(0), Duration::from_millis(1500));
    }

    /// Pictures fed now and then, as a screen that changes gives them: each
    /// change comes out within [`SETTLE`] and a little, rather than waiting
    /// for the next picture; a picture the same as the last is not given
    /// again; the only keyframes are the first and the one asked for, which
    /// starts the encoder again on the last picture; and the stop is not taken
    /// for a failure.
    #[test]
    fn a_live_encoder_settles_each_change_and_makes_a_keyframe_when_asked() {
        if probe_encoder().is_none() {
            eprintln!("skipped: ffmpeg has no H.264 encoder here");
            return;
        }
        let (w, h) = (64u32, 48u32);
        let input = RawInput {
            width: w,
            height: h,
            pix_fmt: "bgr0".to_owned(),
        };
        let settings = Settings {
            display: "1".to_owned(),
            kind: lp_core::screen::SourceKind::Display,
            format: Format::H264,
            audio: None,
            width: w,
            height: h,
            frame_rate: 30,
            viewer: "test".to_owned(),
            cast: false,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let output = Arc::new(Mutex::new(Instant::now()));
        let live = LiveEncoder::start(Encoder::X264, input, &settings, tx, output)
            .expect("the encoder starts");
        let picture = |shade: u8| vec![shade; (w * h * 4) as usize];
        let mut keys = Vec::new();
        // Waits until `n` frames have come in all, then a little more to see
        // that no more come.
        let mut until = |n: usize, keys: &mut Vec<bool>| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while keys.len() < n && Instant::now() < deadline {
                match rx.try_recv() {
                    Ok(Captured::Frame(f)) => keys.push(f.key),
                    Ok(Captured::Audio(_)) => {}
                    Ok(Captured::Ended(r)) => panic!("the share ended: {r:?}"),
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            std::thread::sleep(SETTLE * 3);
            while let Ok(c) = rx.try_recv() {
                match c {
                    Captured::Frame(f) => keys.push(f.key),
                    Captured::Audio(_) => {}
                    Captured::Ended(r) => panic!("the share ended: {r:?}"),
                }
            }
        };
        // Every frame comes out as soon as it is encoded (LP-176): the first
        // picture, then its settling copy.
        live.write(&picture(0)).unwrap();
        until(2, &mut keys);
        assert_eq!(keys, [true, false]);
        // A change, then its settling copy.
        live.write(&picture(90)).unwrap();
        until(4, &mut keys);
        assert_eq!(keys, [true, false, false, false]);
        // The same picture again is encoded but not settled again.
        live.write(&picture(90)).unwrap();
        until(5, &mut keys);
        assert_eq!(keys, [true, false, false, false, false]);
        // A keyframe asked for: a new encoder on the last picture, settled.
        live.keyframe();
        until(7, &mut keys);
        assert_eq!(keys, [true, false, false, false, false, true, false]);
        live.write(&picture(180)).unwrap();
        until(9, &mut keys);
        assert_eq!(
            keys,
            [true, false, false, false, false, true, false, false, false]
        );
        live.stop();
        std::thread::sleep(SETTLE * 4);
        assert!(
            !matches!(rx.try_recv(), Ok(Captured::Ended(_))),
            "a stop is no failure"
        );
    }
}
