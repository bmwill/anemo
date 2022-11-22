use anemo::Request;
use bytes::Bytes;
use tracing::{Level, Span};

use super::DEFAULT_MESSAGE_LEVEL;

/// Trait used to generate [`Span`]s from requests. [`Trace`] wraps all request handling in this
/// span.
///
/// [`Span`]: tracing::Span
/// [`Trace`]: super::Trace
pub trait MakeSpan {
    /// Make a span from a request.
    fn make_span(&mut self, request: &Request<Bytes>) -> Span;
}

impl MakeSpan for Span {
    fn make_span(&mut self, _request: &Request<Bytes>) -> Span {
        self.clone()
    }
}

impl<F> MakeSpan for F
where
    F: FnMut(&Request<Bytes>) -> Span,
{
    fn make_span(&mut self, request: &Request<Bytes>) -> Span {
        self(request)
    }
}

/// The default way [`Span`]s will be created for [`Trace`].
///
/// [`Span`]: tracing::Span
/// [`Trace`]: super::Trace
#[derive(Debug, Clone)]
pub struct DefaultMakeSpan {
    level: Level,
    include_headers: bool,
}

impl DefaultMakeSpan {
    /// Create a new `DefaultMakeSpan`.
    pub fn new() -> Self {
        Self {
            level: DEFAULT_MESSAGE_LEVEL,
            include_headers: false,
        }
    }

    /// Set the [`Level`] used for the [tracing span].
    ///
    /// Defaults to [`Level::DEBUG`].
    ///
    /// [tracing span]: https://docs.rs/tracing/latest/tracing/#spans
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Include request headers on the [`Span`].
    ///
    /// By default headers are not included.
    ///
    /// [`Span`]: tracing::Span
    pub fn include_headers(mut self, include_headers: bool) -> Self {
        self.include_headers = include_headers;
        self
    }
}

impl Default for DefaultMakeSpan {
    fn default() -> Self {
        Self::new()
    }
}

impl MakeSpan for DefaultMakeSpan {
    fn make_span(&mut self, request: &Request<Bytes>) -> Span {
        let headers = self
            .include_headers
            .then(|| tracing::field::debug(request.headers()));
        let peer_id = request
            .peer_id()
            .map(|peer_id| tracing::field::display(peer_id.short_display(4)));
        let direction = request
            .extensions()
            .get::<anemo::Direction>()
            .map(tracing::field::display);

        // This macro is needed, unfortunately, because `tracing::span!` requires the level
        // argument to be static. Meaning we can't just pass `self.level`.
        macro_rules! make_span {
            ($level:expr) => {
                tracing::span!(
                    $level,
                    "request",
                    route = %request.route(),
                    remote_peer_id = peer_id,
                    direction = direction,
                    headers = headers,
                )
            }
        }

        match self.level {
            Level::ERROR => {
                make_span!(Level::ERROR)
            }
            Level::WARN => {
                make_span!(Level::WARN)
            }
            Level::INFO => {
                make_span!(Level::INFO)
            }
            Level::DEBUG => {
                make_span!(Level::DEBUG)
            }
            Level::TRACE => {
                make_span!(Level::TRACE)
            }
        }
    }
}
