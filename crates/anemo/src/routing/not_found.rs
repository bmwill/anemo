use crate::{
    types::response::{IntoResponse, StatusCode},
    Request, Response,
};
use bytes::Bytes;
use std::convert::Infallible;
use tower::Service;

/// A [`Service`] that responds with `404 Not Found` to all requests.
#[derive(Clone, Copy, Debug)]
pub(super) struct NotFound;

impl<B> Service<Request<B>> for NotFound
where
    B: Send + 'static,
{
    type Response = Response<Bytes>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    #[inline]
    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<B>) -> Self::Future {
        std::future::ready(Ok(StatusCode::NotFound.into_response()))
    }
}
