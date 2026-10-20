//! A ceremony in progress, as the host drives it: the offer to show (as a
//! newcomer), the code arrives as an event, the user's answer comes back
//! through [`Pairing::confirm`] or [`Pairing::abort`].

use std::sync::Mutex;

use tokio::sync::oneshot;

/// One ceremony, either role. Dropping it without answering aborts it when
/// the code is shown.
#[derive(Debug, uniffi::Object)]
pub struct Pairing {
    offer: Mutex<Option<String>>,
    answer: Mutex<Option<oneshot::Sender<bool>>>,
    done: Mutex<Option<Result<(), String>>>,
}

impl Pairing {
    pub(crate) fn new(answer: oneshot::Sender<bool>) -> Self {
        Self {
            offer: Mutex::new(None),
            answer: Mutex::new(Some(answer)),
            done: Mutex::new(None),
        }
    }

    pub(crate) fn set_offer(&self, offer: String) {
        *lock(&self.offer) = Some(offer);
    }

    pub(crate) fn finish(&self, result: Result<(), String>) {
        *lock(&self.done) = Some(result);
    }

    fn answer(&self, yes: bool) {
        if let Some(tx) = lock(&self.answer).take() {
            let _ = tx.send(yes);
        }
    }
}

#[uniffi::export]
impl Pairing {
    /// The newcomer's offer text once known (also sent as an event), to show
    /// as a QR code and as text. `None` for an introducer.
    #[must_use]
    pub fn offer(&self) -> Option<String> {
        lock(&self.offer).clone()
    }

    /// The user says the codes match.
    pub fn confirm(&self) {
        self.answer(true);
    }

    /// The user says they differ, or stops the ceremony.
    pub fn abort(&self) {
        self.answer(false);
    }

    /// Whether the ceremony has ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        lock(&self.done).is_some()
    }

    /// Why it failed, once it has ended; `None` while running or on success.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        lock(&self.done).as_ref().and_then(|r| r.clone().err())
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
