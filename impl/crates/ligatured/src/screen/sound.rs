//! A device's own sound for a share (milestone 9, LP-200): what plays on the
//! default output, captured through PulseAudio's interface (PipeWire's on
//! Linux) by ffmpeg, encoded by it as Opus in 20 ms packets and written as
//! Ogg, which the daemon takes the packets out of ([`super::ogg`]). A child
//! process, as the picture's encoder is, so a machine without the pieces
//! simply shares no sound.

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lp_core::screen::AudioFormat;
use tokio::sync::mpsc;

use super::ogg::Reader;
use super::{AudioPacket, Captured};

/// The bit rate Opus is asked for, per channel: enough for a film's sound,
/// a fraction of the picture's.
const BITS_PER_CHANNEL: u32 = 64_000;

/// Whether this machine can capture and encode its sound: ffmpeg with the
/// `libopus` encoder and the `pulse` input, and a default output to listen
/// to. Checked once at start, like the picture's tools.
#[must_use]
pub fn probe() -> bool {
    let has = |args: &[&str], needle: &str| {
        Command::new("ffmpeg")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|text| text.contains(needle))
    };
    has(&["-hide_banner", "-encoders"], " libopus ")
        && has(&["-hide_banner", "-devices"], " pulse ")
        && default_sink().is_some()
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

/// A running sound capture: its ffmpeg, stopped when dropped.
pub struct Sound {
    child: Child,
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
        let mut child = Command::new("ffmpeg")
            .args(capture_args(&source, format))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no stdout from ffmpeg")
        })?;
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
        tracing::info!(%source, %format, "sound capture started");
        Ok(Self { child, stopping })
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
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
    }
}
