use super::{CallbackLayer, MakeCallbackHandler, ResponseFuture};
use anemo::{Request, Response};
use bytes::Bytes;
use std::task::{Context, Poll};
use tower::Service;

/// Middleware that adds callbacks to a [`Service`].
///
/// See the [module docs](crate::callback) for an example.
///
/// [`Service`]: tower::Service
#[derive(Debug, Clone, Copy)]
pub struct Callback<S, M> {
    pub(crate) inner: S,
    pub(crate) make_callback_handler: M,
}

impl<S, M> Callback<S, M> {
    /// Create a new [`Callback`].
    pub fn new(inner: S, make_callback_handler: M) -> Self {
        Self {
            inner,
            make_callback_handler,
        }
    }

    /// Returns a new [`Layer`] that wraps services with a [`CallbackLayer`] middleware.
    ///
    /// [`Layer`]: tower::layer::Layer
    pub fn layer(make_handler: M) -> CallbackLayer<M>
    where
        M: MakeCallbackHandler,
    {
        CallbackLayer::new(make_handler)
    }
}

impl<S, M> Callback<S, M> {
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

impl<S, M> Service<Request<Bytes>> for Callback<S, M>
where
    S: Service<Request<Bytes>, Response = Response<Bytes>>,
    M: MakeCallbackHandler,
{
    type Response = Response<Bytes>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future, M::Handler>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Bytes>) -> Self::Future {
        let handler = self.make_callback_handler.make_handler(&request);

        ResponseFuture {
            inner: self.inner.call(request),
            handler: Some(handler),
        }
    }
}
