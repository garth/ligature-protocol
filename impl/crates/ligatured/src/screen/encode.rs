//! H.264 encoding by ffmpeg, the same on every desktop (milestone 8): the
//! encoder this machine has, the arguments that turn raw frames on stdin into
//! an Annex B stream on stdout with a delimiter before every frame and the
//! parameter sets before every keyframe, and the split of that stream into
//! frames. Linux feeds it from the portal or the screencopy helper (LP-149),
//! the Mac from ScreenCaptureKit (LP-150).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lp_core::screen::StopReason;
use tokio::sync::mpsc;

use crate::screen::{Captured, EncodedFrame, Settings};

/// Bits per pixel per frame the encoder aims for.
const BITS_PER_PIXEL: f64 = 0.08;

/// ffmpeg's expression for a keyframe at the first frame and then at the
/// first frame a second or more after the last keyframe, by wall time: the
/// frames come only when the screen changes, so a count of frames is no clock.
pub const KEYFRAME_EVERY_SECOND: &str = "expr:isnan(prev_forced_t)+gte(t-prev_forced_t,1)";

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
/// the screen changes, at most at the share's rate and at least once a
/// second: each is stamped with the wall time it arrived and encoded once,
/// never repeated to fill the rate, and a keyframe comes every second.
#[must_use]
pub fn encoder_args(encoder: Encoder, input: &RawInput, settings: &Settings) -> Vec<String> {
    let rate = settings.frame_rate.max(1);
    let rate_s = rate.to_string();
    let br = bit_rate(settings.width, settings.height, rate);
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
    let scale = format!("scale={}:{}", settings.width, settings.height);
    match encoder {
        Encoder::Vaapi => args.extend(
            [
                "-vf",
                &format!("{scale},format=nv12,hwupload"),
                "-c:v",
                "h264_vaapi",
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
                "ultrafast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-x264-params",
                "aud=1:repeat-headers=1",
            ]
            .map(str::to_owned),
        ),
    }
    // The longest run without a keyframe, should the forced ones not come.
    let gop = (rate * 2).to_string();
    args.extend(
        [
            "-force_key_frames",
            KEYFRAME_EVERY_SECOND,
            "-g",
            &gop,
            "-bf",
            "0",
            "-fps_mode",
            "passthrough",
            "-b:v",
            &br.to_string(),
            "-maxrate",
            &br.to_string(),
            "-bufsize",
            &(br / 2).to_string(),
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
/// delimiters the encoder puts before every frame.
#[derive(Debug, Default)]
pub struct AccessUnits {
    pending: Vec<u8>,
}

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

/// Whether an access unit holds an IDR slice, so a decoder can start there.
#[must_use]
pub fn is_keyframe(unit: &[u8]) -> bool {
    nal_units(unit).iter().any(|(_, _, t)| *t == NAL_IDR)
}

impl AccessUnits {
    /// Takes more bytes and returns every access unit they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
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
    std::thread::Builder::new()
        .name("screen-encoded".into())
        .spawn(move || {
            let mut encoded = encoded;
            let mut units = AccessUnits::default();
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                match encoded.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for unit in units.push(&buf[..n]) {
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
                }
            }
            if !stopping.load(Ordering::SeqCst) {
                tracing::info!("the encoder's stream ended");
                let _ = frames.send(Captured::Ended(StopReason::Error));
            }
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::screen::Codec;

    #[test]
    fn videotoolbox_gets_delimiters_and_parameter_sets_from_bitstream_filters() {
        let input = RawInput {
            width: 2880,
            height: 1800,
            pix_fmt: "bgra".to_owned(),
        };
        let settings = Settings {
            display: "1".to_owned(),
            codec: Codec::H264,
            width: 1440,
            height: 900,
            frame_rate: 30,
            viewer: "linux".to_owned(),
        };
        let args = encoder_args(Encoder::VideoToolbox, &input, &settings).join(" ");
        assert!(args.contains(
            "-pix_fmt bgra -video_size 2880x1800 -framerate 30 \
             -use_wallclock_as_timestamps 1 -i pipe:0"
        ));
        assert!(args.contains("-vf scale=1440:900,format=nv12 -c:v h264_videotoolbox"));
        assert!(args.contains("-bsf:v dump_extra=freq=keyframe,h264_metadata=aud=insert"));
        assert!(args.contains("-g 60"));
        assert!(args.contains("-bf 0"));
        assert!(args.ends_with("-f h264 pipe:1"));
    }

    /// Frames fed at uneven times, as a screen that changes now and then
    /// gives them: each comes out once, none is added, and keyframes come at
    /// the start and then a second or more apart by wall time.
    #[test]
    fn frames_are_encoded_as_they_come_with_a_keyframe_every_second() {
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
            codec: Codec::H264,
            width: w,
            height: h,
            frame_rate: 30,
            viewer: "test".to_owned(),
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
        // Seconds from the start at which a frame arrives.
        let times = [0.0, 0.05, 0.1, 0.6, 1.2, 1.25, 2.4];
        let start = std::time::Instant::now();
        for (i, t) in times.iter().enumerate() {
            let due = std::time::Duration::from_secs_f64(*t);
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
            let shade = u8::try_from(i * 30).unwrap();
            stdin.write_all(&vec![shade; (w * h * 4) as usize]).unwrap();
        }
        drop(stdin);
        let stream = reader.join().unwrap();
        let _ = child.wait();
        let mut units = AccessUnits::default();
        let mut frames = units.push(&stream);
        frames.extend(units.finish());
        let keys: Vec<f64> = frames
            .iter()
            .zip(times)
            .filter(|(f, _)| is_keyframe(f))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(frames.len(), times.len(), "one frame out for each in");
        assert_eq!(keys, [0.0, 1.2, 2.4], "keyframes a second apart");
    }
}
