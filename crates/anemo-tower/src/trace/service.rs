use crate::classify::Classifier;

use super::{
    DefaultMakeSpan, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, MakeSpan, OnFailure,
    OnRequest, OnResponse, ResponseFuture, TraceLayer,
};
use anemo::{Request, Response};
use bytes::Bytes;
use std::{
    task::{Context, Poll},
    time::Instant,
};
use tower::Service;

/// Middleware that adds high level [tracing] to a [`Service`].
///
/// See the [module docs](crate::trace) for an example.
///
/// [tracing]: https://crates.io/crates/tracing
/// [`Service`]: tower::Service
#[derive(Debug, Clone, Copy)]
pub struct Trace<
    S,
    Classifier,
    MakeSpan = DefaultMakeSpan,
    OnRequest = DefaultOnRequest,
    OnResponse = DefaultOnResponse,
    OnFailure = DefaultOnFailure,
> {
    pub(crate) inner: S,
    pub(crate) classifier: Classifier,
    pub(crate) make_span: MakeSpan,
    pub(crate) on_request: OnRequest,
    pub(crate) on_response: OnResponse,
    pub(crate) on_failure: OnFailure,
}

impl<S, C> Trace<S, C> {
    /// Create a new [`Trace`].
    pub fn new(inner: S, classifier: C) -> Self
    where
        C: Classifier,
    {
        Self {
            inner,
            classifier,
            make_span: DefaultMakeSpan::new(),
            on_request: DefaultOnRequest::default(),
            on_response: DefaultOnResponse::default(),
            on_failure: DefaultOnFailure::default(),
        }
    }

    /// Returns a new [`Layer`] that wraps services with a [`TraceLayer`] middleware.
    ///
    /// [`Layer`]: tower::layer::Layer
    pub fn layer(classifier: C) -> TraceLayer<C>
    where
        C: Classifier,
    {
        TraceLayer::new(classifier)
    }
}

impl<S, Classifier, MakeSpan, OnRequest, OnResponse, OnFailure>
    Trace<S, Classifier, MakeSpan, OnRequest, OnResponse, OnFailure>
{
    /// Gets a reference to the underlying service.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Gets a mutable reference to the underlying service.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Consumes `self`, returning the underlying service.
    pub fn into_inner(self) -> S {
        self.inner
    }

    /// Customize what to do when a request is received.
    ///
    /// `NewOnRequest` is expected to implement [`OnRequest`].
    ///
    /// [`OnRequest`]: super::OnRequest
    pub fn on_request<NewOnRequest>(
        self,
        new_on_request: NewOnRequest,
    ) -> Trace<S, Classifier, MakeSpan, NewOnRequest, OnResponse, OnFailure> {
        Trace {
            inner: self.inner,
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
    ) -> Trace<S, Classifier, MakeSpan, OnRequest, NewOnResponse, OnFailure> {
        Trace {
            inner: self.inner,
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
    ) -> Trace<S, Classifier, MakeSpan, OnRequest, OnResponse, NewOnFailure> {
        Trace {
            inner: self.inner,
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
    ) -> Trace<S, Classifier, NewMakeSpan, OnRequest, OnResponse, OnFailure> {
        Trace {
            inner: self.inner,
            classifier: self.classifier,
            make_span: new_make_span,
            on_request: self.on_request,
            on_response: self.on_response,
            on_failure: self.on_failure,
        }
    }
}

impl<S, ClassifierT, OnRequestT, OnResponseT, OnFailureT, MakeSpanT> Service<Request<Bytes>>
    for Trace<S, ClassifierT, MakeSpanT, OnRequestT, OnResponseT, OnFailureT>
where
    S: Service<Request<Bytes>, Response = Response<Bytes>>,
    S::Error: std::fmt::Display + 'static,
    ClassifierT: Classifier + Clone,
    MakeSpanT: MakeSpan,
    OnRequestT: OnRequest,
    OnResponseT: OnResponse + Clone,
    OnFailureT: OnFailure<ClassifierT::FailureClass> + Clone,
{
    type Response = Response<Bytes>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future, ClassifierT, OnResponseT, OnFailureT>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Bytes>) -> Self::Future {
        let start = Instant::now();

        let span = self.make_span.make_span(&req);

        let future = {
            let _guard = span.enter();
            self.on_request.on_request(&req, &span);
            self.inner.call(req)
        };

        ResponseFuture {
            inner: future,
            classifier: Some(self.classifier.clone()),
            span,
            on_response: Some(self.on_response.clone()),
            on_failure: Some(self.on_failure.clone()),
            start,
        }
    }
}
