//! A real share from a nested compositor (milestone 8, LP-149): a daemon
//! whose capture is `wlr-screencopy` into ffmpeg shares a nested Hyprland's
//! output with a second daemon, which reassembles the frames; the stream is
//! then decoded by ffprobe to check it is the picture asked for. Ignored by
//! default, since it needs a nested compositor on an isolated socket and
//! ffmpeg with `libx264` (see `docs/milestones/08-implementation-decisions.md`,
//! LP-149):
//!
//! ```sh
//! LP_NESTED_DISPLAY=lp-test cargo test -p ligatured --test screen_nested -- --ignored --nocapture
//! ```

#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ligatured::daemon::Daemon;
use ligatured::screen::{ShareControl, Viewed};
use ligatured::screen_linux::LinuxScreen;
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::screen::{Codec, SCREEN_TYPE, ScreenParams, Start, StopReason};
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::Event;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(50)).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a nested Hyprland on LP_NESTED_DISPLAY and ffmpeg"]
async fn a_nested_compositor_s_output_is_shared_encoded_and_decodes() {
    let display = std::env::var("LP_NESTED_DISPLAY").expect("LP_NESTED_DISPLAY");
    let runtime_dir = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());

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
    let (viewer, v_events, _vd) = daemons.pop().unwrap();
    let (sharer, s_events, _sd) = daemons.pop().unwrap();

    let capture = LinuxScreen::start_with(
        sharer.store().clone(),
        runtime_dir,
        Some(display.into()),
        Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_ligatured"))),
        true,
    );
    let sharer_sync = sharer.attach_screen(Some(capture), None);
    let viewer_sync = viewer.attach_screen(
        None,
        Some(ScreenParams {
            views: true,
            ..ScreenParams::default()
        }),
    );
    drive_events(sharer.clone(), s_events);
    drive_events(viewer.clone(), v_events);
    viewer
        .manager()
        .connect(sharer.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    // The probe finds ffmpeg and the nested output, then the sharer
    // advertises that it shares.
    until("the sharer advertises sharing", || {
        viewer.registry().peers().iter().any(|p| {
            p.capabilities
                .iter()
                .any(|d| ScreenParams::from_descriptor(d).is_some_and(|p| p.shares))
        })
    })
    .await;
    assert_eq!(viewer.registry().peers_offering(SCREEN_TYPE).len(), 1);
    let sharer_key = *sharer.binding().public();
    let displays = viewer_sync.displays(&sharer_key).await.unwrap();
    eprintln!("displays: {displays:?}");
    assert_eq!(displays.len(), 1);
    assert!(displays[0].primary);

    let mut watching = viewer_sync
        .watch(
            &sharer_key,
            Start {
                display: None,
                codec: Codec::H264,
                width: 640,
                height: 640,
                frame_rate: 15,
            },
        )
        .await
        .unwrap();
    eprintln!("started: {:?}", watching.started);
    let expect_h = (displays[0].height * 640 / displays[0].width) & !1;
    assert_eq!(
        (watching.started.width, watching.started.height),
        (640, expect_h)
    );

    // Two seconds of frames: the first is a keyframe, keyframes recur.
    let mut stream = Vec::new();
    let (mut frames, mut keys) = (0, 0);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) && frames < 30 {
        match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv()).await {
            Ok(Some(Viewed::Frame {
                seq, key, bytes, ..
            })) => {
                if frames == 0 {
                    assert!(key, "the first frame is a keyframe");
                    assert_eq!(seq, 0);
                }
                frames += 1;
                keys += u32::from(key);
                stream.extend_from_slice(&bytes);
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }
    let rate = f64::from(frames) / started.elapsed().as_secs_f64();
    eprintln!(
        "{frames} frames, {keys} keyframes, {} bytes, {rate:.1} fps",
        stream.len()
    );
    assert!(frames >= 20, "{frames} frames in time");
    assert!(keys >= 2, "a keyframe every second");

    // The stream decodes as the agreed picture.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("share.h264");
    std::fs::write(&path, &stream).unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(&path)
        .output()
        .unwrap();
    let probed = String::from_utf8_lossy(&probe.stdout).trim().to_owned();
    eprintln!("ffprobe: {probed}");
    assert!(
        probed.starts_with(&format!("h264,640,{expect_h},")),
        "{probed}"
    );
    let decoded: u32 = probed.rsplit(',').next().unwrap().parse().unwrap();
    assert_eq!(decoded, frames, "every frame decodes");

    // The viewer stops; the capture's processes go with it.
    watching.control.send(ShareControl::Stop).unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv()).await {
            Ok(Some(Viewed::Stopped(reason))) => {
                assert_eq!(reason, StopReason::Viewer);
                break;
            }
            Ok(Some(Viewed::Frame { .. })) => continue,
            other => panic!("expected the stop, got {other:?}"),
        }
    }
    until("the sharer recorded the stop", || {
        sharer_sync.status().sharing.is_empty()
    })
    .await;
}
