use super::{OnFailure, OnResponse};
use crate::classify::Classifier;
use anemo::Response;
use bytes::Bytes;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tracing::Span;

pin_project! {
    /// Response future for [`Trace`].
    ///
    /// [`Trace`]: super::Trace
    pub struct ResponseFuture<F, Classifier, OnResponse, OnFailure> {
        #[pin]
        pub(crate) inner: F,
        pub(crate) classifier: Option<Classifier>,
        pub(crate) span: Span,
        pub(crate) on_response: Option<OnResponse>,
        pub(crate) on_failure: Option<OnFailure>,
        pub(crate) start: Instant,
    }
}

impl<Fut, E, ClassifierT, OnResponseT, OnFailureT> Future
    for ResponseFuture<Fut, ClassifierT, OnResponseT, OnFailureT>
where
    Fut: Future<Output = Result<Response<Bytes>, E>>,
    E: std::fmt::Display + 'static,
    ClassifierT: Classifier,
    OnResponseT: OnResponse,
    OnFailureT: OnFailure<ClassifierT::FailureClass>,
{
    type Output = Result<Response<Bytes>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.span.enter();
        let result = futures::ready!(this.inner.poll(cx));
        let latency = this.start.elapsed();
        let classifier = this.classifier.take().unwrap();
        let on_response = this.on_response.take().unwrap();
        let mut on_failure = this.on_failure.take().unwrap();

        match &result {
            Ok(response) => {
                on_response.on_response(response, latency, this.span);
                if let Err(failure_class) = classifier.classify_response(response) {
                    on_failure.on_failure(failure_class, latency, this.span);
                }
            }
            Err(err) => {
                let failure_class = classifier.classify_error(err);
                on_failure.on_failure(failure_class, latency, this.span);
            }
        }

        Poll::Ready(result)
    }
}
