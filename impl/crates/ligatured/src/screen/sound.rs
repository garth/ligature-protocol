//! A device's own sound for a share (milestone 9, LP-200): what plays on the
//! default output, captured through PulseAudio's interface (PipeWire's on
//! Linux) by ffmpeg, or handed in as raw samples by the platform
//! (ScreenCaptureKit on a Mac, LP-201), encoded by ffmpeg as Opus in 20 ms
//! packets and written as Ogg, which the daemon takes the packets out of
//! ([`super::ogg`]). A child process, as the picture's encoder is, so a
//! machine without the pieces simply shares no sound.

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lp_core::screen::AudioFormat;
use tokio::sync::mpsc;

use super::ogg::Reader;
use super::{AudioPacket, Captured};

/// The start of the node name of the microphone a presented camera's sound
/// becomes (milestone 10), followed by the sending device's name: never
/// captured as this device's own microphone, or a camera's sound would go
/// back where it came from.
pub const PRESENTED_MICROPHONE: &str = "ligature_microphone.";

/// The bit rate Opus is asked for, per channel: enough for a film's sound,
/// a fraction of the picture's.
const BITS_PER_CHANNEL: u32 = 64_000;

/// Whether ffmpeg prints `needle` for `args`.
fn ffmpeg_has(args: &[&str], needle: &str) -> bool {
    Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|text| text.contains(needle))
}

/// Whether ffmpeg here has the `libopus` encoder, which any sound capture
/// needs.
#[must_use]
pub fn probe_encoder() -> bool {
    ffmpeg_has(&["-hide_banner", "-encoders"], " libopus ")
}

/// Whether this machine can capture and encode its sound through PulseAudio's
/// interface: [`probe_encoder`], ffmpeg's `pulse` input, and a default output
/// to listen to. Checked once at start, like the picture's tools.
#[must_use]
pub fn probe() -> bool {
    probe_encoder()
        && ffmpeg_has(&["-hide_banner", "-devices"], " pulse ")
        && default_sink().is_some()
}

/// Whether this machine can capture and encode its microphone: the Opus
/// encoder, PipeWire's recorder, and a default source to listen to that is
/// not a camera presented here.
#[must_use]
pub fn probe_microphone() -> bool {
    probe_encoder() && runs("pw-record") && default_source().is_some()
}

/// Whether ffmpeg here can play a share's sound into a named output:
/// [`probe_decoder`] and its `pulse` output.
#[must_use]
pub fn probe_player() -> bool {
    probe_decoder() && ffmpeg_has(&["-hide_banner", "-devices"], " pulse ")
}

/// Whether the tool `name` is installed and answers `--version`.
#[must_use]
pub fn runs(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whether ffmpeg here decodes Opus, which a viewer needs to play a share's
/// sound.
#[must_use]
pub fn probe_decoder() -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-decoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|d| d.contains(" opus ") || d.contains(" libopus "))
}

/// The audio formats a device that passes [`probe`] encodes: stereo first,
/// since that is what plays, and mono.
#[must_use]
pub fn encodes() -> Vec<AudioFormat> {
    vec![AudioFormat::OPUS_STEREO, AudioFormat::OPUS_MONO]
}

/// The audio formats a device that passes [`probe_decoder`] plays.
#[must_use]
pub fn decodes() -> Vec<AudioFormat> {
    vec![AudioFormat::OPUS_STEREO, AudioFormat::OPUS_MONO]
}

/// The default output's name, from `pactl`, whose monitor is what plays.
fn default_sink() -> Option<String> {
    let out = Command::new("pactl")
        .arg("get-default-sink")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The default source's name, from `pactl`: the microphone. `None` when
/// there is none, or when it is a presented camera's sound.
fn default_source() -> Option<String> {
    let out = Command::new("pactl")
        .arg("get-default-source")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    own_microphone(&String::from_utf8(out.stdout).ok()?)
}

/// `name` when it names a microphone of this machine's own: not empty, not
/// an output's monitor, not a presented camera's sound.
fn own_microphone(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && !name.ends_with(".monitor") && !name.starts_with(PRESENTED_MICROPHONE))
        .then(|| name.to_owned())
}

/// The ffmpeg that captures `source` and writes Ogg Opus, one 20 ms packet a
/// page, flushed as it goes.
#[must_use]
pub fn capture_args(source: &str, format: AudioFormat) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-f",
        "pulse",
        "-fragment_size",
        "1920",
        "-i",
        source,
        "-ac",
        &format.channels.to_string(),
        "-ar",
        "48000",
        "-c:a",
        "libopus",
        "-b:a",
        &(BITS_PER_CHANNEL * u32::from(format.channels)).to_string(),
        "-frame_duration",
        "20",
        "-application",
        "lowdelay",
        "-page_duration",
        "20000",
        "-flush_packets",
        "1",
        "-f",
        "ogg",
        "pipe:1",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The `pw-record` that reads `source`, and no other in its place at the
/// start or when it goes, as raw 32-bit float samples at 48 kHz on its
/// stdout: what [`pcm_args`]' encoder takes.
#[must_use]
pub fn record_args(source: &str, format: AudioFormat) -> Vec<String> {
    [
        "--target",
        source,
        "-P",
        "{ node.dont-fallback = true node.dont-reconnect = true }",
        "--raw",
        "--format",
        "f32",
        "--rate",
        "48000",
        "--channels",
        &format.channels.to_string(),
        "--latency",
        "20ms",
        "-",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The ffmpeg that takes raw 32-bit float samples at 48 kHz on its stdin
/// and writes Ogg Opus as [`capture_args`] does: for a platform that hands
/// the daemon the samples itself.
#[must_use]
pub fn pcm_args(format: AudioFormat) -> Vec<String> {
    let channels = format.channels.to_string();
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-f",
        "f32le",
        "-ar",
        "48000",
        "-ac",
        &channels,
        "-i",
        "pipe:0",
        "-ac",
        &channels,
        "-ar",
        "48000",
        "-c:a",
        "libopus",
        "-b:a",
        &(BITS_PER_CHANNEL * u32::from(format.channels)).to_string(),
        "-frame_duration",
        "20",
        "-application",
        "lowdelay",
        "-page_duration",
        "20000",
        "-flush_packets",
        "1",
        "-f",
        "ogg",
        "pipe:1",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// A running sound capture: its ffmpeg, stopped when dropped.
pub struct Sound {
    child: Child,
    /// The process that feeds the encoder its samples, where one does.
    feeder: Option<Child>,
    stopping: Arc<AtomicBool>,
}

impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sound")
    }
}

impl Sound {
    /// Starts capturing the default output in `format`, sending each packet
    /// to `frames` as [`Captured::Audio`] with no capture time (the sync's
    /// clock stamps it on arrival, as it does a frame the encoder did not
    /// stamp). A capture that ends on its own is logged and the share goes
    /// on silent.
    ///
    /// # Errors
    ///
    /// Returns the error when there is no default output or ffmpeg cannot
    /// be started.
    pub fn start(
        format: AudioFormat,
        frames: mpsc::UnboundedSender<Captured>,
    ) -> std::io::Result<Self> {
        let sink = default_sink().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no default audio output")
        })?;
        let source = format!("{sink}.monitor");
        let child = Command::new("ffmpeg")
            .args(capture_args(&source, format))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let (sound, _) = Self::reading(child, frames)?;
        tracing::info!(%source, %format, "sound capture started");
        Ok(sound)
    }

    /// Starts capturing the microphone in `format`: the default source, or
    /// `source` when one is named, read by `pw-record` into the encoder
    /// [`Sound::start_pcm`] runs. The recorder names its source and takes no
    /// other, as a camera's reader does: PipeWire would otherwise move it to
    /// another microphone when this one goes, and a share would hear a
    /// device nobody chose. A microphone that goes leaves the share silent.
    ///
    /// # Errors
    ///
    /// Returns the error when there is no microphone or a process cannot be
    /// started.
    pub fn start_microphone(
        format: AudioFormat,
        source: Option<&str>,
        frames: mpsc::UnboundedSender<Captured>,
    ) -> std::io::Result<Self> {
        let source = source
            .map(str::to_owned)
            .or_else(default_source)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no microphone"))?;
        let mut recorder = Command::new("pw-record")
            .args(record_args(&source, format))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let samples = recorder.stdout.take();
        let started = samples
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            .and_then(|samples| {
                Command::new("ffmpeg")
                    .args(pcm_args(format))
                    .stdin(Stdio::from(samples))
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
            })
            .and_then(|child| Self::reading(child, frames));
        match started {
            Ok((mut sound, _)) => {
                sound.feeder = Some(recorder);
                tracing::info!(%source, %format, "microphone capture started");
                Ok(sound)
            }
            Err(e) => {
                let _ = recorder.kill();
                let _ = recorder.wait();
                Err(e)
            }
        }
    }

    /// Starts the encoder for samples the platform hands in: 32-bit float,
    /// 48 kHz, `format.channels` channels interleaved, written to the
    /// returned stdin. The packets go to `frames` as [`Sound::start`]'s do.
    ///
    /// # Errors
    ///
    /// Returns the error when ffmpeg cannot be started.
    pub fn start_pcm(
        format: AudioFormat,
        frames: mpsc::UnboundedSender<Captured>,
    ) -> std::io::Result<(Self, std::process::ChildStdin)> {
        let child = Command::new("ffmpeg")
            .args(pcm_args(format))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let (sound, stdin) = Self::reading(child, frames)?;
        let stdin = stdin.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no stdin to ffmpeg")
        })?;
        tracing::info!(%format, "sound encoder started for the platform's samples");
        Ok((sound, stdin))
    }

    /// Reads the Ogg `child` writes on a thread of its own, the packets to
    /// `frames`; hands back the child's stdin where it has one.
    fn reading(
        mut child: Child,
        frames: mpsc::UnboundedSender<Captured>,
    ) -> std::io::Result<(Self, Option<std::process::ChildStdin>)> {
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no stdout from ffmpeg")
        })?;
        let stdin = child.stdin.take();
        let stopping = Arc::new(AtomicBool::new(false));
        let flag = stopping.clone();
        std::thread::Builder::new()
            .name("screen-sound".into())
            .spawn(move || {
                use std::io::Read as _;
                let mut stdout = stdout;
                let mut reader = Reader::default();
                let mut buf = vec![0u8; 4096];
                let mut packets: u64 = 0;
                loop {
                    let n = match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    for packet in reader.push(&buf[..n]) {
                        packets += 1;
                        if frames
                            .send(Captured::Audio(AudioPacket {
                                at: 0,
                                bytes: packet,
                            }))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                if !flag.load(Ordering::SeqCst) {
                    tracing::warn!(packets, "the sound capture ended; the share goes on silent");
                }
            })?;
        Ok((
            Self {
                child,
                feeder: None,
                stopping,
            },
            stdin,
        ))
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(mut feeder) = self.feeder.take() {
            let _ = feeder.kill();
            let _ = feeder.wait();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capture_asks_for_opus_in_ogg_pages_of_one_packet() {
        let args = capture_args("sink.monitor", AudioFormat::OPUS_STEREO);
        let joined = args.join(" ");
        assert!(joined.contains("-f pulse -fragment_size 1920 -i sink.monitor"));
        assert!(joined.contains("-ac 2 -ar 48000 -c:a libopus -b:a 128000"));
        assert!(joined.contains("-frame_duration 20 -application lowdelay"));
        assert!(joined.contains("-page_duration 20000 -flush_packets 1 -f ogg pipe:1"));
        let mono = capture_args("m", AudioFormat::OPUS_MONO).join(" ");
        assert!(mono.contains("-ac 1 ") && mono.contains("-b:a 64000"));
        assert_eq!(encodes()[0], AudioFormat::OPUS_STEREO);
        assert_eq!(decodes().len(), 2);
        // The platform's own samples: float, 48 kHz, interleaved, on stdin.
        let pcm = pcm_args(AudioFormat::OPUS_STEREO).join(" ");
        assert!(pcm.contains("-f f32le -ar 48000 -ac 2 -i pipe:0"));
        assert!(pcm.contains("-c:a libopus -b:a 128000 -frame_duration 20"));
        assert!(pcm.ends_with("-f ogg pipe:1"));
    }

    #[test]
    fn the_microphone_s_recorder_names_its_source_and_takes_no_other() {
        let args = record_args("alsa_input.mic", AudioFormat::OPUS_MONO).join(" ");
        assert!(args.starts_with(
            "--target alsa_input.mic -P { node.dont-fallback = true node.dont-reconnect = true }"
        ));
        assert!(args.ends_with("--raw --format f32 --rate 48000 --channels 1 --latency 20ms -"));
        // What it writes is what the encoder for a platform's samples reads.
        assert!(
            pcm_args(AudioFormat::OPUS_MONO)
                .join(" ")
                .contains("-f f32le -ar 48000 -ac 1 -i pipe:0")
        );
    }

    #[test]
    fn a_capture_s_ogg_becomes_packets_and_its_end_closes_the_channel() {
        // A stand-in for the encoder: a process that writes three packets as
        // Ogg, one a page, and ends.
        let mut writer = super::super::ogg::Writer::new(7);
        let mut ogg = writer.headers(1);
        for n in 1..=3u8 {
            ogg.extend(writer.packet(&[n; 40]));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sound.ogg");
        std::fs::write(&path, &ogg).unwrap();
        let child = Command::new("cat")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (frames_tx, mut frames) = mpsc::unbounded_channel();
        let (sound, stdin) = Sound::reading(child, frames_tx).unwrap();
        assert!(stdin.is_none(), "this one takes no samples");
        assert_eq!(format!("{sound:?}"), "Sound");
        for n in 1..=3u8 {
            match frames.blocking_recv() {
                Some(Captured::Audio(packet)) => {
                    assert_eq!(packet.bytes, vec![n; 40]);
                    assert_eq!(packet.at, 0, "stamped by the sync on arrival");
                }
                other => panic!("packet {n}: {other:?}"),
            }
        }
        // The headers were not packets, and the capture's end ends the
        // channel without ending the share.
        assert!(frames.blocking_recv().is_none());
        drop(sound);
    }

    #[test]
    fn a_microphone_that_is_not_there_gives_no_sound_rather_than_another_s() {
        // The recorder names its source and takes no other: for a source
        // that does not exist nothing is captured, whether the tools are
        // here (the recorder ends on "defined target not found") or not (it
        // does not start).
        let (frames_tx, mut frames) = mpsc::unbounded_channel();
        let started = Sound::start_microphone(
            AudioFormat::OPUS_MONO,
            Some("ligature_test.no_such_source"),
            frames_tx,
        );
        assert!(
            frames.blocking_recv().is_none(),
            "a packet from a source nobody named"
        );
        drop(started);
    }

    #[test]
    fn a_tool_runs_when_it_is_installed() {
        assert!(runs("true") || runs("cat"));
        assert!(!runs("ligature-no-such-tool"));
    }

    #[test]
    fn a_microphone_is_a_source_of_this_machine_s_own() {
        assert_eq!(
            own_microphone("alsa_input.pci-0000_00_1f.3.analog-stereo\n").as_deref(),
            Some("alsa_input.pci-0000_00_1f.3.analog-stereo")
        );
        // Nothing, what plays, and a camera presented here are not one.
        assert_eq!(own_microphone("\n"), None);
        assert_eq!(own_microphone("alsa_output.speaker.monitor"), None);
        assert_eq!(own_microphone("ligature_microphone.pixel_8_pro"), None);
    }
}
