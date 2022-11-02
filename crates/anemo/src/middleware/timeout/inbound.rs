use crate::{types::response::StatusCode, Request, Response};
use bytes::Bytes;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Sleep;
use tower::{Layer, Service};

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimeoutLayer {
    default_timeout: Option<Duration>,
}

impl TimeoutLayer {
    pub(crate) fn new(default_timeout: Option<Duration>) -> Self {
        TimeoutLayer { default_timeout }
    }
}

impl<S> Layer<S> for TimeoutLayer {
    type Service = Timeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Timeout::new(inner, self.default_timeout)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Timeout<S> {
    inner: S,
    default_timeout: Option<Duration>,
}

impl<S> Timeout<S> {
    pub(crate) fn new(inner: S, default_timeout: Option<Duration>) -> Self {
        Self {
            inner,
            default_timeout,
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for Timeout<S>
where
    S: Service<Request<ReqBody>, Response = Response<Bytes>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let request_timeout = super::try_parse_timeout(req.headers()).unwrap_or_else(|e| {
            tracing::trace!("Error parsing `timeout` header {:?}", e);
            None
        });

        // Use the shorter of the two durations, if either are set
        let timeout_duration = match (request_timeout, self.default_timeout) {
            (None, None) => None,
            (Some(dur), None) => Some(dur),
            (None, Some(dur)) => Some(dur),
            (Some(request), Some(default)) => {
                let shorter_duration = std::cmp::min(request, default);
                Some(shorter_duration)
            }
        };

        ResponseFuture {
            inner: self.inner.call(req),
            sleep: timeout_duration.map(tokio::time::sleep),
        }
    }
}

pin_project! {
    pub(crate) struct ResponseFuture<F> {
        #[pin]
        inner: F,
        #[pin]
        sleep: Option<Sleep>,
    }
}

impl<F, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<Bytes>, E>>,
{
    type Output = Result<Response<Bytes>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(result) = this.inner.poll(cx) {
            return Poll::Ready(result.map_err(Into::into));
        }

        if let Some(sleep) = this.sleep.as_pin_mut() {
            futures::ready!(sleep.poll(cx));
            let response = Response::new(Bytes::new()).with_status(StatusCode::RequestTimeout);
            return Poll::Ready(Ok(response));
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::Timeout;
    use crate::{types::response::StatusCode, Request, Response};
    use bytes::Bytes;
    use std::{convert::Infallible, time::Duration};
    use tower::{service_fn, ServiceBuilder, ServiceExt};

    enum ExpectedResult {
        Success,
        Failure,
    }

    async fn timeout_service_test(
        timeout: Option<Duration>,
        sleep_duration: Duration,
        request_timeout: Option<Duration>,
        expected_result: ExpectedResult,
    ) {
        let service = ServiceBuilder::new()
            .layer_fn(move |s| Timeout::new(s, timeout))
            .service(service_fn(move |_req: Request<Bytes>| async move {
                tokio::time::sleep(sleep_duration).await;
                Ok::<_, Infallible>(Response::new(Bytes::new()))
            }));

        let mut request = Request::new(Bytes::new());
        if let Some(request_timeout) = request_timeout {
            request.set_timeout(request_timeout);
        }

        let response = service.oneshot(request).await.unwrap();

        match expected_result {
            ExpectedResult::Success => {
                assert_eq!(response.status(), StatusCode::Success);
            }
            ExpectedResult::Failure => {
                assert_eq!(response.status(), StatusCode::RequestTimeout);
            }
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn no_timeout() {
        let timeout = None;
        let sleep_duration = Duration::from_secs(1);
        let request_timeout = None;
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Success,
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn default_timeout() {
        let timeout = Some(Duration::from_secs(2));
        let sleep_duration = Duration::from_secs(1);
        let request_timeout = None;
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Success,
        )
        .await;

        let timeout = Some(Duration::from_secs(2));
        let sleep_duration = Duration::from_secs(4);
        let request_timeout = None;
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Failure,
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn request_timeout() {
        let timeout = None;
        let sleep_duration = Duration::from_secs(1);
        let request_timeout = Some(Duration::from_secs(2));
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Success,
        )
        .await;

        let timeout = None;
        let sleep_duration = Duration::from_secs(4);
        let request_timeout = Some(Duration::from_secs(2));
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Failure,
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn default_and_request_timeout() {
        let timeout = Some(Duration::from_secs(2));
        let sleep_duration = Duration::from_secs(1);
        let request_timeout = Some(Duration::from_secs(2));
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Success,
        )
        .await;

        let timeout = Some(Duration::from_secs(4));
        let sleep_duration = Duration::from_secs(1);
        let request_timeout = Some(Duration::from_secs(2));
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Success,
        )
        .await;

        // Lesser timeout is respected
        let timeout = Some(Duration::from_secs(10));
        let sleep_duration = Duration::from_secs(5);
        let request_timeout = Some(Duration::from_secs(1));
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Failure,
        )
        .await;

        // Lesser timeout is respected
        let timeout = Some(Duration::from_secs(1));
        let sleep_duration = Duration::from_secs(5);
        let request_timeout = Some(Duration::from_secs(10));
        timeout_service_test(
            timeout,
            sleep_duration,
            request_timeout,
            ExpectedResult::Failure,
        )
        .await;
    }
}
