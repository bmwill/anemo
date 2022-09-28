use super::super::LatencyUnit;
use super::DEFAULT_ERROR_LEVEL;
use std::time::Duration;
use tracing::{Level, Span};

/// Trait used to tell [`Trace`] what to do when a request fails.
///
/// See the [module docs](../trace/index.html#on_failure) for details on exactly when the
/// `on_failure` callback is called.
///
/// [`Trace`]: super::Trace
pub trait OnFailure {
    /// Do the thing.
    ///
    /// `latency` is the duration since the request was received.
    ///
    /// `span` is the `tracing` [`Span`], corresponding to this request, produced by the closure
    /// passed to [`TraceLayer::make_span_with`]. It can be used to [record field values][record]
    /// that weren't known when the span was created.
    ///
    /// [`Span`]: https://docs.rs/tracing/latest/tracing/span/index.html
    /// [record]: https://docs.rs/tracing/latest/tracing/span/struct.Span.html#method.record
    /// [`TraceLayer::make_span_with`]: crate::trace::TraceLayer::make_span_with
    fn on_failure(&mut self, error: &dyn std::fmt::Display, latency: Duration, span: &Span);
}

impl OnFailure for () {
    #[inline]
    fn on_failure(&mut self, _: &dyn std::fmt::Display, _: Duration, _: &Span) {}
}

impl<F> OnFailure for F
where
    F: FnMut(&dyn std::fmt::Display, Duration, &Span),
{
    fn on_failure(&mut self, error: &dyn std::fmt::Display, latency: Duration, span: &Span) {
        self(error, latency, span)
    }
}

/// The default [`OnFailure`] implementation used by [`Trace`].
///
/// [`Trace`]: super::Trace
#[derive(Clone, Debug)]
pub struct DefaultOnFailure {
    level: Level,
    latency_unit: LatencyUnit,
}

impl Default for DefaultOnFailure {
    fn default() -> Self {
        Self {
            level: DEFAULT_ERROR_LEVEL,
            latency_unit: LatencyUnit::Millis,
        }
    }
}

impl DefaultOnFailure {
    /// Create a new `DefaultOnFailure`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the [`Level`] used for [tracing events].
    ///
    /// Defaults to [`Level::ERROR`].
    ///
    /// [tracing events]: https://docs.rs/tracing/latest/tracing/#events
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Set the [`LatencyUnit`] latencies will be reported in.
    ///
    /// Defaults to [`LatencyUnit::Millis`].
    pub fn latency_unit(mut self, latency_unit: LatencyUnit) -> Self {
        self.latency_unit = latency_unit;
        self
    }
}

impl OnFailure for DefaultOnFailure {
    fn on_failure(&mut self, error: &dyn std::fmt::Display, latency: Duration, _: &Span) {
        // This macro is needed, unfortunately, because `tracing::event!` requires the level
        // argument to be static. Meaning we can't just pass `self.level`.
        macro_rules! log_response {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    error = tracing::field::display(error),
                    latency = %self.latency_unit.display(latency),
                    "response failed",
                )
            };
        }

        match self.level {
            Level::ERROR => {
                log_response!(Level::ERROR)
            }
            Level::WARN => {
                log_response!(Level::WARN)
            }
            Level::INFO => {
                log_response!(Level::INFO)
            }
            Level::DEBUG => {
                log_response!(Level::DEBUG)
            }
            Level::TRACE => {
                log_response!(Level::TRACE)
            }
        }
    }
}
