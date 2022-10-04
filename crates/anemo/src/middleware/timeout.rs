use crate::types::header;
use crate::types::HeaderMap;
use crate::Request;
use pin_project_lite::pin_project;
use std::convert::TryInto;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Sleep;
use tower::Service;

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
    S: Service<Request<ReqBody>>,
    S::Error: Into<crate::Error>,
{
    type Response = S::Response;
    type Error = crate::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let request_timeout = try_parse_timeout(req.headers()).unwrap_or_else(|e| {
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

impl<F, Res, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Res, E>>,
    E: Into<crate::Error>,
{
    type Output = Result<Res, crate::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(result) = this.inner.poll(cx) {
            return Poll::Ready(result.map_err(Into::into));
        }

        if let Some(sleep) = this.sleep.as_pin_mut() {
            futures::ready!(sleep.poll(cx));
            return Poll::Ready(Err(TimeoutExpired(()).into()));
        }

        Poll::Pending
    }
}

/// Tries to parse the `timeout` header if it is present. If we fail to parse, returns
/// the value we attempted to parse.
///
/// The value of the `timeout` header is expected to in nanoseconds encoded encoded as an u64
fn try_parse_timeout(headers: &HeaderMap) -> Result<Option<Duration>, &str> {
    match headers.get(header::TIMEOUT) {
        Some(val) => {
            let nanoseconds = val.parse::<u64>().map_err(|_| val.as_ref())?;
            let duration = Duration::from_nanos(nanoseconds);

            Ok(Some(duration))
        }
        None => Ok(None),
    }
}

/// Converts a duration to nanoseconds as an u64 suitable to be used in the `timeout` header field.
pub(crate) fn duration_to_timeout(duration: Duration) -> String {
    let nanoseconds: u64 = duration.as_nanos().try_into().unwrap_or(u64::MAX);
    nanoseconds.to_string()
}

/// Error returned if a request didn't complete within the configured timeout.
#[derive(Debug)]
pub struct TimeoutExpired(());

impl fmt::Display for TimeoutExpired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timeout expired")
    }
}

// std::error::Error only requires a type to impl Debug and Display
impl std::error::Error for TimeoutExpired {}

#[cfg(test)]
mod tests {
    use super::Timeout;
    use super::TimeoutExpired;
    use crate::Request;
    use crate::Response;
    use bytes::Bytes;
    use std::convert::Infallible;
    use std::time::Duration;
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

        let result = service.oneshot(request).await;

        match expected_result {
            ExpectedResult::Success => {
                result.unwrap();
            }
            ExpectedResult::Failure => {
                result.unwrap_err().downcast::<TimeoutExpired>().unwrap();
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
