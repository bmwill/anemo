use super::{DefaultMakeSpan, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, Trace};
use tower::Layer;

/// [`Layer`] that adds high level [tracing] to a [`Service`].
///
/// See the [module docs](crate::trace) for more details.
///
/// [`Layer`]: tower::layer::Layer
/// [tracing]: https://crates.io/crates/tracing
/// [`Service`]: tower::Service
#[derive(Debug, Copy, Clone)]
pub struct TraceLayer<
    MakeSpan = DefaultMakeSpan,
    OnRequest = DefaultOnRequest,
    OnResponse = DefaultOnResponse,
    OnFailure = DefaultOnFailure,
> {
    pub(crate) make_span: MakeSpan,
    pub(crate) on_request: OnRequest,
    pub(crate) on_response: OnResponse,
    pub(crate) on_failure: OnFailure,
}

impl TraceLayer {
    /// Create a new [`TraceLayer`].
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            make_span: DefaultMakeSpan::new(),
            on_failure: DefaultOnFailure::default(),
            on_request: DefaultOnRequest::default(),
            on_response: DefaultOnResponse::default(),
        }
    }
}

impl<MakeSpan, OnRequest, OnResponse, OnFailure>
    TraceLayer<MakeSpan, OnRequest, OnResponse, OnFailure>
{
    /// Customize what to do when a request is received.
    ///
    /// `NewOnRequest` is expected to implement [`OnRequest`].
    ///
    /// [`OnRequest`]: super::OnRequest
    pub fn on_request<NewOnRequest>(
        self,
        new_on_request: NewOnRequest,
    ) -> TraceLayer<MakeSpan, NewOnRequest, OnResponse, OnFailure> {
        TraceLayer {
            on_request: new_on_request,
            on_failure: self.on_failure,
            make_span: self.make_span,
            on_response: self.on_response,
        }
    }

    /// Customize what to do when a response has been produced.
    ///
    /// `NewOnResponse` is expected to implement [`OnResponse`].
    ///
    /// [`OnResponse`]: super::OnResponse
    pub fn on_response<NewOnResponse>(
        self,
        new_on_response: NewOnResponse,
    ) -> TraceLayer<MakeSpan, OnRequest, NewOnResponse, OnFailure> {
        TraceLayer {
            on_response: new_on_response,
            on_request: self.on_request,
            on_failure: self.on_failure,
            make_span: self.make_span,
        }
    }

    /// Customize what to do when a response has been classified as a failure.
    ///
    /// `NewOnFailure` is expected to implement [`OnFailure`].
    ///
    /// [`OnFailure`]: super::OnFailure
    pub fn on_failure<NewOnFailure>(
        self,
        new_on_failure: NewOnFailure,
    ) -> TraceLayer<MakeSpan, OnRequest, OnResponse, NewOnFailure> {
        TraceLayer {
            on_failure: new_on_failure,
            on_request: self.on_request,
            make_span: self.make_span,
            on_response: self.on_response,
        }
    }

    /// Customize how to make [`Span`]s that all request handling will be wrapped in.
    ///
    /// `NewMakeSpan` is expected to implement [`MakeSpan`].
    ///
    /// [`MakeSpan`]: super::MakeSpan
    /// [`Span`]: tracing::Span
    pub fn make_span_with<NewMakeSpan>(
        self,
        new_make_span: NewMakeSpan,
    ) -> TraceLayer<NewMakeSpan, OnRequest, OnResponse, OnFailure> {
        TraceLayer {
            make_span: new_make_span,
            on_request: self.on_request,
            on_failure: self.on_failure,
            on_response: self.on_response,
        }
    }
}

impl<S, MakeSpan, OnRequest, OnResponse, OnFailure> Layer<S>
    for TraceLayer<MakeSpan, OnRequest, OnResponse, OnFailure>
where
    MakeSpan: Clone,
    OnRequest: Clone,
    OnResponse: Clone,
    OnFailure: Clone,
{
    type Service = Trace<S, MakeSpan, OnRequest, OnResponse, OnFailure>;

    fn layer(&self, inner: S) -> Self::Service {
        Trace {
            inner,
            make_span: self.make_span.clone(),
            on_request: self.on_request.clone(),
            on_response: self.on_response.clone(),
            on_failure: self.on_failure.clone(),
        }
    }
}
