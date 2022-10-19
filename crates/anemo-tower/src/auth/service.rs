use super::{AuthorizeRequest, RequireAuthorizationLayer, ResponseFuture};
use anemo::{Request, Response};
use bytes::Bytes;
use std::task::{Context, Poll};
use tower::Service;

/// Middleware that adds authorization to a [`Service`].
///
/// See the [module docs](crate::auth) for an example.
///
/// [`Service`]: tower::Service
#[derive(Debug, Clone, Copy)]
pub struct RequireAuthorization<S, A> {
    pub(crate) inner: S,
    pub(crate) auth: A,
}

impl<S, A> RequireAuthorization<S, A> {
    /// Create a new [`RequireAuthorization`].
    pub fn new(inner: S, auth: A) -> Self {
        Self { inner, auth }
    }

    /// Returns a new [`Layer`] that wraps services with a [`RequireAuthorizationLayer`] middleware.
    ///
    /// [`Layer`]: tower::layer::Layer
    pub fn layer(auth: A) -> RequireAuthorizationLayer<A>
    where
        A: AuthorizeRequest,
    {
        RequireAuthorizationLayer::new(auth)
    }
}

impl<S, A> RequireAuthorization<S, A> {
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
}

impl<S, A> Service<Request<Bytes>> for RequireAuthorization<S, A>
where
    S: Service<Request<Bytes>, Response = Response<Bytes>>,
    A: AuthorizeRequest,
{
    type Response = Response<Bytes>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<Bytes>) -> Self::Future {
        match self.auth.authorize(&mut request) {
            Ok(()) => ResponseFuture::future(self.inner.call(request)),
            Err(response) => ResponseFuture::invalid_auth(response),
        }
    }
}
