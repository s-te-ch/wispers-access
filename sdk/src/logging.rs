//! The SDK's log lines, handed to the app through a callback. Rust apps
//! install a `tracing` subscriber of their own instead.

use crate::SdkError;
use std::sync::Arc;
use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// Where the SDK's log lines go. Called from the SDK's threads.
#[uniffi::export(with_foreign)]
pub trait LogSink: Send + Sync {
    /// `target` is the Rust module the line came from; `message` carries
    /// the event's fields already rendered.
    fn log(&self, level: LogLevel, target: String, message: String);
}

/// Routes the SDK's lines at `level` and above to `sink`, and its
/// dependencies' warnings and errors. Once per process; a second call, or
/// a call after a Rust app installed its own subscriber, fails.
#[uniffi::export]
pub fn install_log_sink(sink: Arc<dyn LogSink>, level: LogLevel) -> Result<(), SdkError> {
    let filter = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target("wispers_access_sdk", LevelFilter::from(level));
    tracing_subscriber::registry()
        .with(filter)
        .with(SinkLayer { sink })
        .try_init()
        .map_err(|e| SdkError::Internal(format!("installing the log sink: {e}")))
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => LevelFilter::ERROR,
            LogLevel::Warn => LevelFilter::WARN,
            LogLevel::Info => LevelFilter::INFO,
            LogLevel::Debug => LevelFilter::DEBUG,
            LogLevel::Trace => LevelFilter::TRACE,
        }
    }
}

struct SinkLayer {
    sink: Arc<dyn LogSink>,
}

impl<S: Subscriber> Layer<S> for SinkLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut line = Line::default();
        event.record(&mut line);
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::TRACE => LogLevel::Trace,
        };
        self.sink
            .log(level, event.metadata().target().to_owned(), line.render());
    }
}

/// An event's fields as one line: the message first, then `key=value`.
#[derive(Default)]
struct Line {
    message: String,
    fields: Vec<String>,
}

impl Line {
    fn render(self) -> String {
        let mut line = self.message;
        for field in self.fields {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&field);
        }
        line
    }
}

impl tracing::field::Visit for Line {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push(format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields.push(format!("{}={}", field.name(), value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_puts_the_message_first() {
        let mut line = Line::default();
        line.fields.push("share=rt".to_owned());
        line.message = "connecting".to_owned();
        assert_eq!(line.render(), "connecting share=rt");
        assert_eq!(Line::default().render(), "");
    }
}
