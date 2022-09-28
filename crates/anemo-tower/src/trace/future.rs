use super::{OnFailure, OnResponse};
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
    pub struct ResponseFuture<F, OnResponse, OnFailure> {
        #[pin]
        pub(crate) inner: F,
        pub(crate) span: Span,
        pub(crate) on_response: Option<OnResponse>,
        pub(crate) on_failure: Option<OnFailure>,
        pub(crate) start: Instant,
    }
}

impl<Fut, E, OnResponseT, OnFailureT> Future for ResponseFuture<Fut, OnResponseT, OnFailureT>
where
    Fut: Future<Output = Result<Response<Bytes>, E>>,
    E: std::fmt::Display + 'static,
    OnResponseT: OnResponse,
    OnFailureT: OnFailure,
{
    type Output = Result<Response<Bytes>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.span.enter();
        let result = futures::ready!(this.inner.poll(cx));
        let latency = this.start.elapsed();

        match &result {
            Ok(response) => {
                this.on_response
                    .take()
                    .unwrap()
                    .on_response(response, latency, this.span);
            }
            Err(err) => {
                this.on_failure
                    .take()
                    .unwrap()
                    .on_failure(err, latency, this.span);
            }
        }

        Poll::Ready(result)
    }
}
