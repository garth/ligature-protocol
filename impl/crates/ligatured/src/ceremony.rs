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
