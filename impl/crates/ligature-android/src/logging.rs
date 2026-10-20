//! Log lines go to the host, which writes them to logcat. No native logging
//! crate: a `tracing` layer formats each event and calls [`crate::Host::log`]
//! on whichever host is current, so a node restarted in the same process
//! (Android keeps the process) logs to the new host.

use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::{LogLevel, SharedHost};

static HOST: Mutex<Option<SharedHost>> = Mutex::new(None);
static INSTALLED: OnceLock<()> = OnceLock::new();

/// Makes `host` the destination of log lines and installs the layer once.
/// `debug` lowers the threshold from `info` to `debug`; the threshold of the
/// first installation is the one that stays for the process.
pub fn install(host: SharedHost, debug: bool) {
    *HOST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    INSTALLED.get_or_init(|| {
        let level = if debug {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        };
        let filter = tracing_subscriber::filter::LevelFilter::from_level(level);
        // Fails only if the process already has a global subscriber (a test
        // harness, for example); then that one keeps the lines.
        let _ = tracing_subscriber::registry()
            .with(HostLayer.with_filter(filter))
            .try_init();
    });
}

/// Forgets the host, so a stopped node's host can be released.
pub fn clear(host: &SharedHost) {
    let mut slot = HOST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.as_ref().is_some_and(|h| Arc::ptr_eq(h, host)) {
        *slot = None;
    }
}

struct HostLayer;

impl<S: tracing::Subscriber> Layer<S> for HostLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let host = HOST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(host) = host else {
            return;
        };
        let mut line = Line::default();
        event.record(&mut line);
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::TRACE => LogLevel::Trace,
        };
        host.log(level, event.metadata().target().to_owned(), line.text);
    }
}

#[derive(Default)]
struct Line {
    text: String,
}

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let message = format!("{value:?}");
            if self.text.is_empty() {
                self.text = message;
            } else {
                self.text = format!("{message} {}", self.text);
            }
        } else {
            if !self.text.is_empty() {
                self.text.push(' ');
            }
            self.text.push_str(&format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.record_debug(field, &format_args!("{value}"));
        } else {
            if !self.text.is_empty() {
                self.text.push(' ');
            }
            self.text.push_str(&format!("{}={value}", field.name()));
        }
    }
}
