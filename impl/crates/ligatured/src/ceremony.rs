//! Pumps a `pairing` stream through the ceremony state machines. Spec
//! chapter 02 section 4; ADR 0029.
//!
//! The state machines in `lp_identity::pairing` know the rules; this module
//! moves their messages over a [`FramedStream`], derives the short code from
//! the connection's exporter, shows it to the user and waits for their
//! confirmation.

use lp_core::Error;
use lp_core::wire::Timestamp;
use lp_identity::pairing::{
    Abort, Confirmed, Done, EXPORTER_LABEL, EXPORTER_LEN, Grant, Hello, Introduced, Introducer,
    Joined, Newcomer, Ready, short_code, types,
};
use lp_transport::quic::Connection;
use lp_transport::stream::FramedStream;
use tokio::sync::{mpsc, oneshot};

/// How the ceremony talks to the person: the code to show, and their answer.
#[derive(Debug)]
pub struct Prompt {
    /// Receives the six-digit code once it is known.
    pub code: mpsc::Sender<String>,
    /// Resolves to whether the codes matched.
    pub confirm: oneshot::Receiver<bool>,
}

fn code_for(conn: &Connection, context: &[u8; 32]) -> Result<String, Error> {
    let exporter = conn.export_keying_material(EXPORTER_LABEL, context, EXPORTER_LEN)?;
    short_code(&exporter)
}

async fn recv_typed(stream: &mut FramedStream) -> Result<(String, Vec<u8>), Error> {
    let body = stream
        .recv_bytes()
        .await?
        .ok_or_else(|| Error::Chain("pairing stream closed".to_owned()))?;
    let t = lp_core::frame::peek_type(&body)?;
    if t.as_str() == types::ABORT {
        let abort: Abort = lp_core::frame::decode(&body)?;
        return Err(Error::Chain(format!("peer aborted: {}", abort.reason)));
    }
    Ok((t.0, body))
}

fn expect<T: serde::de::DeserializeOwned>(
    (t, body): (String, Vec<u8>),
    want: &str,
) -> Result<T, Error> {
    if t != want {
        return Err(Error::Chain(format!("expected {want}, got {t}")));
    }
    lp_core::frame::decode(&body)
}

async fn send_abort(stream: &mut FramedStream, reason: &str) {
    let _ = stream.send(&Abort::new(reason)).await;
    stream.finish();
}

/// Runs the newcomer's side on an accepted `pairing` stream from `conn`.
///
/// # Errors
///
/// Returns the first failure; the stream has been told to abort.
pub async fn run_newcomer(
    conn: &Connection,
    mut stream: FramedStream,
    mut newcomer: Newcomer,
    prompt: Prompt,
) -> Result<Joined, Error> {
    let result = newcomer_inner(conn, &mut stream, &mut newcomer, prompt).await;
    if let Err(e) = &result {
        send_abort(&mut stream, &e.to_string()).await;
    }
    result
}

async fn newcomer_inner(
    conn: &Connection,
    stream: &mut FramedStream,
    newcomer: &mut Newcomer,
    prompt: Prompt,
) -> Result<Joined, Error> {
    let hello: Hello = expect(recv_typed(stream).await?, types::HELLO)?;
    let ready = newcomer.on_hello(*conn.peer(), &hello)?;
    stream.send(&ready).await?;
    let code = code_for(conn, &newcomer.exporter_context()?)?;
    let _ = prompt.code.send(code).await;
    // Wait for the user, unless the introducer goes away first.
    let ok = tokio::select! {
        answer = prompt.confirm => answer.unwrap_or(false),
        _ = conn.raw().closed() => return Err(newcomer.abort("connection lost")),
    };
    if !ok {
        return Err(newcomer.abort("user said the codes differ"));
    }
    stream.send(&newcomer.confirm()?).await?;
    // The introducer's own confirmation may arrive before its grant.
    let grant: Grant = loop {
        let msg = recv_typed(stream).await?;
        if msg.0 == types::CONFIRMED {
            continue;
        }
        break expect(msg, types::GRANT)?;
    };
    let (joined, done) = newcomer.on_grant(grant)?;
    stream.send(&done).await?;
    stream.finish();
    Ok(joined)
}

/// Runs the introducer's side on a `pairing` stream it opened on `conn`,
/// whose peer must be the offer's binding key.
///
/// # Errors
///
/// Returns the first failure; the stream has been told to abort.
pub async fn run_introducer(
    conn: &Connection,
    mut stream: FramedStream,
    mut introducer: Introducer,
    prompt: Prompt,
    now: Timestamp,
) -> Result<Introduced, Error> {
    let result = introducer_inner(conn, &mut stream, &mut introducer, prompt, now).await;
    if let Err(e) = &result {
        send_abort(&mut stream, &e.to_string()).await;
    }
    result
}

async fn introducer_inner(
    conn: &Connection,
    stream: &mut FramedStream,
    introducer: &mut Introducer,
    prompt: Prompt,
    now: Timestamp,
) -> Result<Introduced, Error> {
    if conn.peer() != introducer.expected_peer() {
        return Err(introducer.abort("connected to a key that is not in the offer"));
    }
    stream.send(&introducer.hello()).await?;
    let ready: Ready = expect(recv_typed(stream).await?, types::READY)?;
    introducer.on_ready(ready)?;
    let code = code_for(conn, &introducer.exporter_context())?;
    let _ = prompt.code.send(code).await;

    // Wait for both our user and the peer, in either order.
    let mut confirm = prompt.confirm;
    let mut i_answered = false;
    loop {
        if let Some(grant) = introducer.grant(now)? {
            stream.send(&grant).await?;
            break;
        }
        if i_answered {
            let msg = recv_typed(stream).await?;
            let _: Confirmed = expect(msg, types::CONFIRMED)?;
            introducer.on_confirmed()?;
            continue;
        }
        tokio::select! {
            answer = &mut confirm => {
                i_answered = true;
                if !answer.unwrap_or(false) {
                    return Err(introducer.abort("user said the codes differ"));
                }
                stream.send(&introducer.confirm()?).await?;
            }
            msg = recv_typed(stream) => {
                let _: Confirmed = expect(msg?, types::CONFIRMED)?;
                introducer.on_confirmed()?;
            }
        }
    }
    let _: Done = expect(recv_typed(stream).await?, types::DONE)?;
    let introduced = introducer.on_done()?;
    stream.finish();
    Ok(introduced)
}

/// An open ceremony on a control connection.
#[derive(Debug)]
pub struct PairingHandle {
    /// Where the user's answer goes, once.
    pub confirm: Option<oneshot::Sender<bool>>,
}

/// Handles the `pair.*` control requests. `pair.join` and `pair.introduce`
/// run the ceremony in a task that writes events and its final response to
/// `out`; `pair.confirm` and `pair.abort` answer at once.
pub async fn handle_control(
    daemon: &std::sync::Arc<crate::daemon::Daemon>,
    state: &mut crate::control::ConnectionState,
    request: crate::control::Request,
    out: mpsc::Sender<crate::control::Outgoing>,
) -> Option<crate::control::Response> {
    use crate::control::{Empty, ErrorBody, Event, Outgoing, Request, Response, types};
    let id = request.id;
    match request.request_type.as_str() {
        types::PAIR_CONFIRM | types::PAIR_ABORT => {
            let answer = request.request_type.as_str() == types::PAIR_CONFIRM;
            let Some(pairing) = request.pairing else {
                return Some(Response::error(
                    id,
                    ErrorBody::new("bad-request", "pairing id required"),
                ));
            };
            match state
                .pairings
                .get_mut(&pairing)
                .and_then(|h| h.confirm.take())
            {
                Some(tx) => {
                    let _ = tx.send(answer);
                    Some(
                        Response::ok(id, &Empty {})
                            .unwrap_or_else(|e| Response::error(id, ErrorBody::from_error(&e))),
                    )
                }
                None => Some(Response::error(
                    id,
                    ErrorBody::new("not-found", "no pairing waiting for an answer"),
                )),
            }
        }
        types::PAIR_JOIN | types::PAIR_INTRODUCE => {
            let (confirm_tx, confirm_rx) = oneshot::channel();
            let (code_tx, mut code_rx) = mpsc::channel::<String>(1);
            state.pairings.insert(
                id,
                PairingHandle {
                    confirm: Some(confirm_tx),
                },
            );
            let prompt = Prompt {
                code: code_tx,
                confirm: confirm_rx,
            };
            // Forward the code as an event.
            let events = out.clone();
            tokio::spawn(async move {
                while let Some(code) = code_rx.recv().await {
                    let _ = events
                        .send(Outgoing::Event(Event {
                            event_type: lp_core::wire::TypeName::new(types::PAIR_CODE),
                            id,
                            offer: None,
                            code: Some(code),
                            progress: None,
                        }))
                        .await;
                }
            });
            let daemon = daemon.clone();
            let join = request.request_type.as_str() == types::PAIR_JOIN;
            let req: Request = request;
            tokio::spawn(async move {
                let result = if join {
                    run_join(&daemon, id, &out, prompt).await
                } else {
                    run_introduce(&daemon, &req, prompt).await
                };
                let response = match result {
                    Ok(()) => Response::ok(id, &Empty {})
                        .unwrap_or_else(|e| Response::error(id, ErrorBody::from_error(&e))),
                    Err(e) => Response::error(id, ErrorBody::from_error(&e)),
                };
                let _ = out.send(Outgoing::Response(response)).await;
            });
            None
        }
        other => Some(Response::error(
            id,
            ErrorBody::new("unknown-type", format!("unknown request type {other}")),
        )),
    }
}

/// Runs the newcomer's whole ceremony for `daemon`: makes the offer and sends
/// its text on `offer`, waits for an introducer to dial, runs the exchange with
/// `prompt`, persists the outcome and treats the introducer's connection as a
/// trusted peer's. For hosts without the control socket (the Android app) as
/// much as for `pair.join`.
///
/// # Errors
///
/// Returns [`Error::AlreadyInitialised`], [`Error::Chain`] if a ceremony is
/// already waiting, or the ceremony's failure.
pub async fn join(
    daemon: &std::sync::Arc<crate::daemon::Daemon>,
    prompt: Prompt,
    offer: mpsc::Sender<String>,
) -> Result<(), Error> {
    if daemon.is_initialised() {
        return Err(Error::AlreadyInitialised);
    }
    let mut streams = daemon.expect_pairing()?;
    let _ceremony = daemon.begin_ceremony();
    let result = async {
        let newcomer = Newcomer::new(
            daemon.binding().clone(),
            daemon.principal().clone(),
            daemon.device_name(),
            crate::daemon::platform(),
            Timestamp::now(),
        )?;
        let text = newcomer.offer(daemon.dialable_addrs()?).to_text();
        offer
            .send(text)
            .await
            .map_err(|_| Error::Io("nobody to show the offer to".to_owned()))?;
        let (conn, stream) = streams
            .recv()
            .await
            .ok_or_else(|| Error::Chain("pairing cancelled".to_owned()))?;
        let joined = run_newcomer(&conn, stream, newcomer, prompt).await?;
        daemon.join(&joined)?;
        // The introducer's connection is now a trusted peer's: gossip and
        // announce on it as if it had just connected.
        daemon.on_trusted(&conn).await;
        Ok(())
    }
    .await;
    daemon.stop_pairing();
    result
}

/// Runs the introducer's whole ceremony for `daemon` against the offer text a
/// newcomer showed: dials it, runs the exchange with `prompt`, persists the
/// outcome and treats the newcomer's connection as a trusted peer's.
///
/// # Errors
///
/// Returns [`Error::Decode`] for a malformed offer, [`Error::Chain`] if this
/// device is not an introducer, [`Error::Transport`] if no address answers
/// with the offered key, or the ceremony's failure.
pub async fn introduce(
    daemon: &std::sync::Arc<crate::daemon::Daemon>,
    offer_text: &str,
    make_introducer: bool,
    prompt: Prompt,
) -> Result<(), Error> {
    use lp_identity::pairing::{Offer, PURPOSE};
    let offer = Offer::parse(offer_text)?;
    let _ceremony = daemon.begin_ceremony();
    let self_signing = daemon
        .self_signing()?
        .ok_or_else(|| Error::Chain("this device is not an introducer".to_owned()))?;
    let person = daemon.person().ok_or(Error::NotInitialised)?;
    let person_fact = daemon.person_fact()?;
    let gossip = daemon.all_facts()?;

    // Dial the offered addresses in order until one answers with the right key.
    let mut conn = None;
    let mut last = Error::Transport("offer has no address".to_owned());
    for addr in &offer.addrs {
        let attempt = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            daemon.manager().connect(*addr),
        )
        .await
        .unwrap_or_else(|_| Err(Error::Transport(format!("{addr}: no answer within 3 s"))));
        match attempt {
            Ok(c) if c.peer() == &offer.binding => {
                conn = Some(c);
                break;
            }
            Ok(c) => {
                last = Error::Chain(format!("{addr} answered with a key not in the offer"));
                daemon.manager().drop_peer(c.peer());
            }
            Err(e) => last = e,
        }
    }
    let conn = conn.ok_or(last)?;
    let stream = lp_transport::manager::open_on(&conn, PURPOSE).await?;
    let introducer = Introducer::new(
        self_signing,
        person,
        person_fact,
        *daemon.binding().public(),
        offer,
        make_introducer,
        gossip,
    );
    let introduced = run_introducer(&conn, stream, introducer, prompt, Timestamp::now()).await?;
    introduced.persist(daemon.store())?;
    // The newcomer is now one of ours on this very connection.
    daemon.on_trusted(&conn).await;
    // Facts otherwise travel only at the start of a session, so push the
    // newcomer's to every peer already connected (LP-80).
    daemon.gossip_all().await;
    Ok(())
}

async fn run_join(
    daemon: &std::sync::Arc<crate::daemon::Daemon>,
    id: u64,
    out: &mpsc::Sender<crate::control::Outgoing>,
    prompt: Prompt,
) -> Result<(), Error> {
    use crate::control::{Event, Outgoing, types};
    let (offer_tx, mut offer_rx) = mpsc::channel::<String>(1);
    let events = out.clone();
    tokio::spawn(async move {
        while let Some(offer) = offer_rx.recv().await {
            let _ = events
                .send(Outgoing::Event(Event {
                    event_type: lp_core::wire::TypeName::new(types::PAIR_OFFER),
                    id,
                    offer: Some(offer),
                    code: None,
                    progress: None,
                }))
                .await;
        }
    });
    join(daemon, prompt, offer_tx).await
}

async fn run_introduce(
    daemon: &std::sync::Arc<crate::daemon::Daemon>,
    request: &crate::control::Request,
    prompt: Prompt,
) -> Result<(), Error> {
    let text = request
        .offer
        .as_deref()
        .ok_or_else(|| Error::Decode("pair.introduce needs the offer".to_owned()))?;
    introduce(daemon, text, request.introducer.unwrap_or(true), prompt).await
}
