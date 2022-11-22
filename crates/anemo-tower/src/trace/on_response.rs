use super::{super::LatencyUnit, DEFAULT_MESSAGE_LEVEL};
use anemo::Response;
use bytes::Bytes;
use std::time::Duration;
use tracing::{Level, Span};

/// Trait used to tell [`Trace`] what to do when a response has been produced.
///
/// See the [module docs](../trace/index.html#on_response) for details on exactly when the
/// `on_response` callback is called.
///
/// [`Trace`]: super::Trace
pub trait OnResponse {
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
    fn on_response(self, response: &Response<Bytes>, latency: Duration, span: &Span);
}

impl OnResponse for () {
    #[inline]
    fn on_response(self, _: &Response<Bytes>, _: Duration, _: &Span) {}
}

impl<F> OnResponse for F
where
    F: FnOnce(&Response<Bytes>, Duration, &Span),
{
    fn on_response(self, response: &Response<Bytes>, latency: Duration, span: &Span) {
        self(response, latency, span)
    }
}

/// The default [`OnResponse`] implementation used by [`Trace`].
///
/// [`Trace`]: super::Trace
#[derive(Clone, Debug)]
pub struct DefaultOnResponse {
    level: Level,
    latency_unit: LatencyUnit,
    include_headers: bool,
}

impl Default for DefaultOnResponse {
    fn default() -> Self {
        Self {
            level: DEFAULT_MESSAGE_LEVEL,
            latency_unit: LatencyUnit::Millis,
            include_headers: false,
        }
    }
}

impl DefaultOnResponse {
    /// Create a new `DefaultOnResponse`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the [`Level`] used for [tracing events].
    ///
    /// Please note that while this will set the level for the tracing events
    /// themselves, it might cause them to lack expected information, like
    /// request method or path. You can address this using
    /// [`DefaultMakeSpan::level`].
    ///
    /// Defaults to [`Level::DEBUG`].
    ///
    /// [tracing events]: https://docs.rs/tracing/latest/tracing/#events
    /// [`DefaultMakeSpan::level`]: crate::trace::DefaultMakeSpan::level
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

    /// Include response headers on the [`Event`].
    ///
    /// By default headers are not included.
    ///
    /// [`Event`]: tracing::Event
    pub fn include_headers(mut self, include_headers: bool) -> Self {
        self.include_headers = include_headers;
        self
    }
}

impl OnResponse for DefaultOnResponse {
    fn on_response(self, response: &Response<Bytes>, latency: Duration, _: &Span) {
        let headers = self
            .include_headers
            .then(|| tracing::field::debug(response.headers()));

        // This macro is needed, unfortunately, because `tracing::event!` requires the level
        // argument to be static. Meaning we can't just pass `self.level`.
        macro_rules! log_response {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    latency = %self.latency_unit.display(latency),
                    status = response.status().to_u16(),
                    headers = headers,
                    "finished processing request",
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
