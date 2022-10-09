use super::{DefaultMakeSpan, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, Trace};
use crate::classify::{Classifier, StatusInRangeAsFailures};
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
    Classifier,
    MakeSpan = DefaultMakeSpan,
    OnRequest = DefaultOnRequest,
    OnResponse = DefaultOnResponse,
    OnFailure = DefaultOnFailure,
> {
    pub(crate) classifier: Classifier,
    pub(crate) make_span: MakeSpan,
    pub(crate) on_request: OnRequest,
    pub(crate) on_response: OnResponse,
    pub(crate) on_failure: OnFailure,
}

impl<C> TraceLayer<C> {
    /// Create a new [`TraceLayer`] using the given [`Classifier`].
    pub fn new(classifier: C) -> Self
    where
        C: Classifier,
    {
        Self {
            classifier,
            make_span: DefaultMakeSpan::new(),
            on_failure: DefaultOnFailure::default(),
            on_request: DefaultOnRequest::default(),
            on_response: DefaultOnResponse::default(),
        }
    }
}

impl TraceLayer<StatusInRangeAsFailures> {
    /// Create a new [`TraceLayer`] using [`StatusInRangeAsFailures`] configured to classify server
    /// errors as failures.
    pub fn new_for_server_errors() -> Self {
        Self::new(StatusInRangeAsFailures::new_for_server_errors())
    }

    /// Create a new [`TraceLayer`] using [`StatusInRangeAsFailures`] configured to classify client
    /// and server errors as failures.
    pub fn new_for_client_and_server_errors() -> Self {
        Self::new(StatusInRangeAsFailures::new_for_client_and_server_errors())
    }
}

impl<Classifier, MakeSpan, OnRequest, OnResponse, OnFailure>
    TraceLayer<Classifier, MakeSpan, OnRequest, OnResponse, OnFailure>
{
    /// Customize what to do when a request is received.
    ///
    /// `NewOnRequest` is expected to implement [`OnRequest`].
    ///
    /// [`OnRequest`]: super::OnRequest
    pub fn on_request<NewOnRequest>(
        self,
        new_on_request: NewOnRequest,
    ) -> TraceLayer<Classifier, MakeSpan, NewOnRequest, OnResponse, OnFailure> {
        TraceLayer {
            classifier: self.classifier,
            make_span: self.make_span,
            on_request: new_on_request,
            on_response: self.on_response,
            on_failure: self.on_failure,
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
    ) -> TraceLayer<Classifier, MakeSpan, OnRequest, NewOnResponse, OnFailure> {
        TraceLayer {
            classifier: self.classifier,
            make_span: self.make_span,
            on_request: self.on_request,
            on_response: new_on_response,
            on_failure: self.on_failure,
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
    ) -> TraceLayer<Classifier, MakeSpan, OnRequest, OnResponse, NewOnFailure> {
        TraceLayer {
            classifier: self.classifier,
            make_span: self.make_span,
            on_request: self.on_request,
            on_response: self.on_response,
            on_failure: new_on_failure,
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
    ) -> TraceLayer<Classifier, NewMakeSpan, OnRequest, OnResponse, OnFailure> {
        TraceLayer {
            classifier: self.classifier,
            make_span: new_make_span,
            on_request: self.on_request,
            on_response: self.on_response,
            on_failure: self.on_failure,
        }
    }
}

impl<S, Classifier, MakeSpan, OnRequest, OnResponse, OnFailure> Layer<S>
    for TraceLayer<Classifier, MakeSpan, OnRequest, OnResponse, OnFailure>
where
    Classifier: Clone,
    MakeSpan: Clone,
    OnRequest: Clone,
    OnResponse: Clone,
    OnFailure: Clone,
{
    type Service = Trace<S, Classifier, MakeSpan, OnRequest, OnResponse, OnFailure>;

    fn layer(&self, inner: S) -> Self::Service {
        Trace {
            inner,
            classifier: self.classifier.clone(),
            make_span: self.make_span.clone(),
            on_request: self.on_request.clone(),
            on_response: self.on_response.clone(),
            on_failure: self.on_failure.clone(),
        }
    }
}
