//! The Linux camera end to end on a real PipeWire session (milestone 10,
//! LP-207): a test camera node and a test microphone node are shared by one
//! daemon and presented by another, and the camera and microphone nodes the
//! presenter makes are read back by a second pipeline.
//!
//! Ignored by default: it needs PipeWire with its session manager,
//! GStreamer's PipeWire elements, ffmpeg and a notification server, and it
//! makes nodes in the session's own PipeWire for as long as it runs:
//!
//! ```sh
//! cargo test -p ligatured --test camera_live -- --ignored --nocapture
//! ```
//!
//! It never opens a camera or a microphone of the machine's own: the sharer
//! lists the test camera alone and captures the test source alone, every
//! reader forbids PipeWire's fallback to the default device, and nothing is
//! read before the test nodes are seen to exist.
#![cfg(target_os = "linux")]

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ligatured::camera_linux::{LinuxCamera, LinuxPresenter, PRESENTED_CAMERA, pipewire_args};
use ligatured::daemon::Daemon;
use ligatured::screen::sound::PRESENTED_MICROPHONE;
use ligatured::screen::{ScreenSync, Settings, ShareControl, Viewed, Watching};
use lp_core::camera::CAMERA_TYPE;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::screen::{AudioFormat, Format, ScreenParams, SourceKind, Start, StopReason};
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::Event;

const NO_FALLBACK: &str =
    "stream-properties=\"props,node.dont-fallback=true,node.dont-reconnect=true\"";

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, seconds: u64, mut check: impl FnMut() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(seconds), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

fn drive_events(d: Arc<Daemon>, mut events: tokio::sync::mpsc::Receiver<Event>) {
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                Event::Connected(conn) => {
                    if d.recognise(conn.peer())
                        .map(|r| r.is_trusted())
                        .unwrap_or(false)
                    {
                        let d = d.clone();
                        tokio::spawn(async move { d.on_trusted(&conn).await });
                    }
                }
                Event::Disconnected(peer) => d.on_disconnected(&peer),
            }
        }
    });
}

/// The test's helper processes, killed when the test ends however it ends.
struct Helpers(Vec<Child>);

impl Drop for Helpers {
    fn drop(&mut self) {
        // The last started first: the tone's player ends before the output
        // it plays into, so it is never moved to the machine's speakers.
        for child in self.0.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The names of PipeWire's nodes of `class`.
fn nodes(class: &str) -> Vec<String> {
    let out = Command::new("pw-dump").output().expect("pw-dump runs");
    let objects: serde_json::Value = serde_json::from_slice(&out.stdout).expect("pw-dump's JSON");
    objects
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let props = o.get("info")?.get("props")?;
            (props.get("media.class")?.as_str()? == class)
                .then(|| props.get("node.name")?.as_str().map(str::to_owned))?
        })
        .collect()
}

/// A GStreamer pipeline run as a helper.
fn gst(pipeline: &[&str], properties: &str) -> Child {
    Command::new("gst-launch-1.0")
        .arg("-q")
        .args(pipeline)
        .arg(properties)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("gst-launch-1.0 starts")
}

/// The properties of a test camera node.
fn camera_properties(name: &str) -> String {
    format!(
        "stream-properties=\"props,media.class=Video/Source,media.role=Camera,\
         node.name={name},node.description=\\\"LP test camera\\\"\""
    )
}

/// A test camera in the session's PipeWire: a white square moving across
/// black, 1280 by 720 at 30 a second, given no priority so the session's
/// default camera stays what it was. It is built as the daemon builds a
/// presented camera, raw pictures on a pipe into `rawvideoparse`, a leaky
/// queue and an unsynchronised sink, which is what makes it serve one reader
/// after another: the node first, then the ffmpeg that draws its pictures.
fn test_camera(name: &str) -> Vec<Child> {
    let mut pictures = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-re"])
        .args(["-f", "lavfi", "-i", "color=black:s=1280x720:r=30"])
        .args(["-f", "lavfi", "-i", "color=white:s=120x120:r=30"])
        .args(["-filter_complex", "[0][1]overlay=x='mod(240*t,1160)':y=300"])
        .args(["-pix_fmt", "yuv420p", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("ffmpeg starts");
    let node = Command::new("gst-launch-1.0")
        .args(["-q", "fdsrc", "fd=0", "blocksize=1382400", "!"])
        .args(["rawvideoparse", "format=i420", "width=1280", "height=720"])
        .args(["framerate=30/1", "!", "queue", "leaky=downstream"])
        .args([
            "max-size-buffers=2",
            "max-size-bytes=0",
            "max-size-time=0",
            "!",
        ])
        .args(["pipewiresink", "mode=provide", "sync=false"])
        .arg(camera_properties(name))
        .stdin(Stdio::from(pictures.stdout.take().unwrap()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("gst-launch-1.0 starts");
    vec![node, pictures]
}

/// A test camera that stops for good when a reader holds it up: a live
/// source straight into the sink, whose buffers run out when they are not
/// given back in time. Some cameras and screens behave so; this one does
/// reliably, and is left damaged for every later reader too.
fn fragile_camera(name: &str) -> Child {
    gst(
        &[
            "videotestsrc",
            "is-live=true",
            "pattern=ball",
            "!",
            "video/x-raw,format=I420,width=1280,height=720,framerate=30/1",
            "!",
            "pipewiresink",
            "mode=provide",
        ],
        &camera_properties(name),
    )
}

/// What a reader of the node `target`, and of no other, writes within ten
/// seconds: `tail` is the pipeline after `pipewiresrc`.
async fn read_back(target: &str, buffers: u32, tail: &str) -> Vec<u8> {
    let mut args = vec![
        "10".to_owned(),
        "gst-launch-1.0".to_owned(),
        "-q".to_owned(),
        "pipewiresrc".to_owned(),
        format!("target-object=\"{target}\""),
        NO_FALLBACK.to_owned(),
        format!("num-buffers={buffers}"),
    ];
    args.extend(tail.split_whitespace().map(str::to_owned));
    tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .expect("the reader runs")
            .stdout
    })
    .await
    .unwrap()
}

/// The frames, and the keyframes among them, that arrive within `seconds`.
async fn frames_within(watching: &mut Watching, seconds: u64) -> (usize, usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let (mut frames, mut keys) = (0, 0);
    while let Ok(viewed) = tokio::time::timeout_at(deadline, watching.frames.recv()).await {
        match viewed.expect("the share still runs") {
            Viewed::Frame { key, .. } => {
                frames += 1;
                keys += usize::from(key);
            }
            Viewed::Audio { .. } => {}
            Viewed::Stopped(reason) => panic!("the share stopped: {reason}"),
        }
    }
    (frames, keys)
}

/// Reads whole pictures of `frame` bytes from `from` on a thread of its
/// own, saying each one, and reads nothing while `paused` is set.
fn count_pictures(
    mut from: impl std::io::Read + Send + 'static,
    frame: usize,
    paused: Arc<std::sync::atomic::AtomicBool>,
) -> std::sync::mpsc::Receiver<()> {
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut picture = vec![0u8; frame];
        loop {
            if paused.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if from.read_exact(&mut picture).is_err() || said.send(()).is_err() {
                return;
            }
        }
    });
    heard
}

/// How many pictures are said within `seconds`, however long the next one
/// takes to come.
fn pictures_within(pictures: &std::sync::mpsc::Receiver<()>, seconds: u64) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut count = 0;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        if pictures.recv_timeout(left).is_err() {
            break;
        }
        count += 1;
    }
    count
}

fn start(camera: &str, audio: bool) -> Start {
    Start {
        display: Some(camera.to_owned()),
        window: None,
        formats: Vec::new(),
        width: 1920,
        height: 1080,
        frame_rate: 60,
        audio: if audio {
            vec![AudioFormat::OPUS_MONO]
        } else {
            Vec::new()
        },
    }
}

struct Pair {
    sharer: Arc<Daemon>,
    sharer_sync: Arc<ScreenSync>,
    viewer_sync: Arc<ScreenSync>,
    viewer: Arc<Daemon>,
    _dirs: Vec<tempfile::TempDir>,
}

/// A sharer that lists the test camera alone and hears the test source
/// alone, and a viewer that presents.
async fn pair(camera: &str, microphone: &str) -> Pair {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["sharer", "viewer"] {
        let dir = tempfile::tempdir().unwrap();
        let loc = Locations::resolve(Some(dir.path()), None);
        let (d, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
        daemons.push((d, events, dir));
    }
    let roles: Vec<_> = daemons
        .iter()
        .enumerate()
        .map(|(i, (d, _, _))| {
            facts::role_fact(
                &alice,
                Role::Binding,
                *d.binding().public(),
                true,
                at(1 + i as i64),
            )
            .unwrap()
        })
        .collect();
    for (d, _, _) in &daemons {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let (viewer, viewer_events, viewer_dir) = daemons.pop().unwrap();
    let (sharer, sharer_events, sharer_dir) = daemons.pop().unwrap();
    let sharer_sync = sharer.attach_camera(
        Some(LinuxCamera::start_with(
            Some(camera.to_owned()),
            Some(microphone.to_owned()),
        )),
        None,
        None,
    );
    let viewer_sync = viewer.attach_camera(
        None,
        Some(ScreenParams {
            views: true,
            decodes: vec![Format::H264],
            audio_decodes: vec![AudioFormat::OPUS_MONO],
            max_width: 1280,
            max_height: 720,
            max_frame_rate: 30,
            ..ScreenParams::default()
        }),
        Some(LinuxPresenter::start()),
    );
    drive_events(sharer.clone(), sharer_events);
    drive_events(viewer.clone(), viewer_events);
    viewer
        .manager()
        .connect(sharer.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    Pair {
        sharer,
        sharer_sync,
        viewer_sync,
        viewer,
        _dirs: vec![sharer_dir, viewer_dir],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PipeWire, GStreamer, ffmpeg and a notification server; makes test nodes in the session's PipeWire"]
async fn a_test_camera_and_microphone_are_shared_and_presented_as_nodes_that_read_back() {
    let pid = std::process::id();
    let camera = format!("lp_live_test_camera_{pid}");
    let microphone = format!("lp_live_test_microphone_{pid}");
    let mut helpers = Helpers(Vec::new());
    // A camera that shows a white square moving across black, and a microphone
    // that hears a 440 Hz tone. Neither is given a priority, so the
    // session's defaults stay what they were, and nothing is shared before
    // both are seen to exist: a share must find them, not a device of the
    // machine's own.
    helpers.0.extend(test_camera(&camera));
    until("the test camera exists", 10, || {
        nodes("Video/Source").contains(&camera)
    })
    .await;
    // The microphone is a loopback pair, as a presented one is: an output a
    // tone is played into, and the source at its other end. The tone starts
    // only once its output is there, or it would play on the speakers.
    let feed = format!("lp_live_test_feed_{pid}");
    helpers.0.push(
        Command::new("pw-loopback")
            .arg(format!(
                "--capture-props={{ media.class = Audio/Sink node.name = {feed} \
                 node.description = \"LP test feed\" audio.position = [ MONO ] }}"
            ))
            .arg(format!(
                "--playback-props={{ media.class = Audio/Source node.name = {microphone} \
                 node.description = \"LP test microphone\" audio.position = [ MONO ] }}"
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pw-loopback starts"),
    );
    until("the test microphone exists", 10, || {
        nodes("Audio/Sink").contains(&feed) && nodes("Audio/Source").contains(&microphone)
    })
    .await;
    helpers.0.push(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-re"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", "120", "-ac", "1", "-af", "volume=0.5"])
            .args(["-f", "pulse", "-device", &feed, "-buffer_duration", "40"])
            .arg("LP test tone")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ffmpeg starts"),
    );

    let p = pair(&camera, &microphone).await;
    let sharer_key = *p.sharer.binding().public();
    until("the camera is listed and the viewer presents", 30, || {
        p.sharer_sync.local_cameras().len() == 1
            && p.sharer_sync.status().shares
            && p.viewer_sync.status().presents
            && p.viewer
                .registry()
                .peers_offering(CAMERA_TYPE)
                .contains(&sharer_key)
    })
    .await;
    let listed = p.viewer_sync.cameras(&sharer_key).await.unwrap();
    assert_eq!(listed.len(), 1, "the test camera alone: {listed:?}");
    assert_eq!(listed[0].id, camera);
    assert_eq!(listed[0].name.as_deref(), Some("LP test camera"));
    assert_eq!((listed[0].width, listed[0].height), (1280, 720));
    assert_eq!(listed[0].frame_rates, [30]);

    // Presented with its microphone: a camera node and a source node named
    // for the sharing device.
    let presenting = p
        .viewer_sync
        .present(&sharer_key, start(&camera, true))
        .await
        .expect("the camera is presented");
    assert_eq!(presenting.started.source_id(), camera);
    assert_eq!(
        (presenting.started.width, presenting.started.height),
        (1280, 720)
    );
    assert_eq!(presenting.started.frame_rate, 30);
    assert_eq!(presenting.started.audio, Some(AudioFormat::OPUS_MONO));
    // Named for the sharing device, which these two daemons know by its
    // key alone.
    let presented =
        |class: &str, prefix: &str| nodes(class).into_iter().find(|n| n.starts_with(prefix));
    until("the presented nodes exist", 10, || {
        presented("Video/Source", PRESENTED_CAMERA).is_some()
            && presented("Audio/Source", PRESENTED_MICROPHONE).is_some()
    })
    .await;
    let presented_camera = presented("Video/Source", PRESENTED_CAMERA).unwrap();
    let presented_microphone = presented("Audio/Source", PRESENTED_MICROPHONE).unwrap();
    println!("presented as {presented_camera} and {presented_microphone}");
    // The sharer does not list what a daemon presents as a camera of its
    // own, and its one camera is still the test camera.
    assert_eq!(p.sharer_sync.local_cameras().len(), 1);

    // The camera reads back: the pictures the test camera shows, moving.
    let frame = 1280 * 720 * 4;
    let pictures = read_back(
        &presented_camera,
        45,
        "! videoconvert ! video/x-raw,format=BGRx ! fdsink fd=1 sync=false",
    )
    .await;
    let count = pictures.len() / frame;
    println!("read back {count} pictures of the presented camera");
    assert!(count >= 30, "only {count} pictures read back");
    let first = &pictures[..frame];
    let last = &pictures[(count - 1) * frame..count * frame];
    let dark = first
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] < 48 && px[1] < 48 && px[2] < 48)
        .count();
    let bright = first
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] > 200 && px[1] > 200 && px[2] > 200)
        .count();
    println!(
        "the first picture: {dark} dark pixels and {bright} bright of {}",
        frame / 4
    );
    assert!(dark > frame / 4 * 9 / 10, "the black field");
    assert!(bright > 5000, "the white square");
    assert_ne!(first, last, "the square moves");

    // The microphone reads back: the tone the test source plays, through a
    // recorder that takes no other source.
    let recorded = presented_microphone.clone();
    let samples = tokio::task::spawn_blocking(move || {
        Command::new("timeout")
            .args(["4", "pw-record", "--target", &recorded, "-P"])
            .arg("{ node.dont-fallback = true node.dont-reconnect = true }")
            .args([
                "--raw",
                "--format",
                "s16",
                "--rate",
                "48000",
                "--channels",
                "1",
                "-",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .expect("the recorder runs")
            .stdout
    })
    .await
    .unwrap();
    let samples: Vec<i16> = samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    assert!(samples.len() > 48_000 * 2, "only {} samples", samples.len());
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let crossings = samples
        .windows(2)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count();
    let tone = crossings as f64 / 2.0 / (samples.len() as f64 / 48_000.0);
    println!(
        "read back {} samples of the presented microphone: peak {peak}, about {tone:.0} Hz",
        samples.len()
    );
    assert!(peak > 1000, "silence: peak {peak}");
    assert!((425.0..455.0).contains(&tone), "not the tone: {tone:.0} Hz");

    // The viewer stops: both nodes go, and the sharer shares nothing.
    assert_eq!(p.viewer_sync.stop_with(&sharer_key, StopReason::Viewer), 1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), presenting.ended)
            .await
            .expect("the share ends")
            .unwrap(),
        StopReason::Viewer
    );
    until("the presented nodes are gone", 10, || {
        !nodes("Video/Source").contains(&presented_camera)
            && !nodes("Audio/Source").contains(&presented_microphone)
    })
    .await;
    until("the sharer shares nothing", 10, || {
        p.sharer_sync.status().sharing.is_empty()
    })
    .await;

    // A viewer that asks for a keyframe as the share starts, as the phone's
    // decoder does when its surface is ready, gets one and the share goes
    // on at its rate, and the camera is as good for the next reader.
    for round in 1..=1 {
        let mut watching = p
            .viewer_sync
            .watch(&sharer_key, start(&camera, false))
            .await
            .expect("the camera is watched");
        watching.control.send(ShareControl::Keyframe).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        watching.control.send(ShareControl::Keyframe).unwrap();
        let (early, _) = frames_within(&mut watching, 3).await;
        let (frames, _) = frames_within(&mut watching, 3).await;
        println!(
            "watch {round}: {early} frames in the three seconds around the requests, {frames} in the next three"
        );
        assert!(
            frames >= 60,
            "watch {round}: the camera stalled after the requests: {frames} frames in three seconds"
        );
        // And one asked for once the share runs.
        watching.control.send(ShareControl::Keyframe).unwrap();
        let (frames, keys) = frames_within(&mut watching, 3).await;
        assert!(keys >= 1, "watch {round}: no keyframe when asked");
        assert!(
            frames >= 45,
            "watch {round}: {frames} frames after a later request"
        );
        watching.control.send(ShareControl::Stop).unwrap();
        until("the sharer shares nothing after the watch", 10, || {
            p.sharer_sync.status().sharing.is_empty()
        })
        .await;
        // The camera is as good for the next reader as it was: a share
        // whose encoder was started again leaves it with all its buffers.
        let after = read_back(
            &camera,
            400,
            "! videoconvert ! video/x-raw,format=BGRx ! fdsink fd=1 sync=false",
        )
        .await
        .len()
            / frame;
        println!("watch {round}: a reader of the camera itself then gets {after} pictures");
        assert!(
            after >= 100,
            "watch {round}: the camera was left short of buffers: {after} pictures in ten seconds"
        );
    }

    // A camera that goes away ends its share: its reader takes no other
    // camera in its place.
    let presenting = p
        .viewer_sync
        .present(&sharer_key, start(&camera, false))
        .await
        .expect("the camera is presented again");
    assert_eq!(presenting.started.audio, None);
    until("the presented camera exists again", 10, || {
        nodes("Video/Source").contains(&presented_camera)
    })
    .await;
    assert!(
        !nodes("Audio/Source").contains(&presented_microphone),
        "no microphone without sound"
    );
    // The first helper started is the test camera's node.
    let mut test_camera = helpers.0.remove(0);
    test_camera.kill().unwrap();
    test_camera.wait().unwrap();
    let reason = tokio::time::timeout(Duration::from_secs(15), presenting.ended)
        .await
        .expect("the share ends when its camera goes")
        .unwrap();
    println!("the share ended with {reason} when its camera went away");
    assert_eq!(reason, StopReason::Error);
    until("the presented camera is gone", 10, || {
        !nodes("Video/Source").contains(&presented_camera)
    })
    .await;
    until("the sharer lists no camera", 30, || {
        p.sharer_sync.local_cameras().is_empty() && !p.sharer_sync.status().shares
    })
    .await;
}

/// The camera's reader does not hold the camera up when what it feeds stops
/// reading for a while, as the daemon does while an encoder is started again
/// for a keyframe. Held up, the reader keeps the camera's buffers, the camera
/// runs out of them, and its pictures stop for good: found with the phone,
/// while chasing casts that stopped after some twenty frames. The reader is
/// the daemon's own, and the camera one that does stall when it is held up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PipeWire and GStreamer; makes a test camera in the session's PipeWire"]
async fn the_camera_s_reader_is_not_held_up_by_what_it_feeds() {
    let camera = format!("lp_live_test_paused_{}", std::process::id());
    let mut helpers = Helpers(Vec::new());
    helpers.0.push(fragile_camera(&camera));
    until("the test camera exists", 10, || {
        nodes("Video/Source").contains(&camera)
    })
    .await;
    let settings = Settings {
        display: camera.clone(),
        kind: SourceKind::Camera,
        format: Format::H264,
        audio: None,
        width: 1280,
        height: 720,
        frame_rate: 30,
        viewer: "the test".to_owned(),
        cast: false,
    };
    let frame = 1280 * 720 * 4;
    let counted = tokio::task::spawn_blocking(move || {
        let reader = |settings: &Settings| {
            Command::new("gst-launch-1.0")
                .args(pipewire_args(&settings.display, settings))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("the reader starts")
        };
        let mut first = reader(&settings);
        let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pictures = count_pictures(first.stdout.take().unwrap(), frame, paused.clone());
        let before = pictures_within(&pictures, 2);
        paused.store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1500));
        paused.store(false, std::sync::atomic::Ordering::SeqCst);
        let after = pictures_within(&pictures, 4);
        let _ = first.kill();
        let _ = first.wait();
        // And the camera is as good for the next reader.
        let mut second = reader(&settings);
        let running = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pictures = count_pictures(second.stdout.take().unwrap(), frame, running);
        let next = pictures_within(&pictures, 4);
        let _ = second.kill();
        let _ = second.wait();
        (before, after, next)
    })
    .await
    .unwrap();
    println!(
        "the reader gave {} pictures in two seconds, {} in the four after a pause of 1.5 s, and the next reader {} in four",
        counted.0, counted.1, counted.2
    );
    assert!(counted.0 >= 15, "only {} pictures at the start", counted.0);
    assert!(
        counted.1 >= 60,
        "the camera stalled after the pause: {} pictures in four seconds",
        counted.1
    );
    assert!(
        counted.2 >= 60,
        "the camera was left stalled for the next reader: {} pictures in four seconds",
        counted.2
    );
}
