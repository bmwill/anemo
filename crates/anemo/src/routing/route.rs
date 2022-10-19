use crate::{Request, Response};
use bytes::Bytes;
use std::{convert::Infallible, fmt};
use tower::{
    util::{BoxCloneService, Oneshot},
    Service, ServiceExt,
};

/// How routes are stored inside a [`Router`](super::Router).
///
/// You normally shouldn't need to care about this type. It's used in
/// [`Router::route_layer`](super::Router::route_layer).
#[derive(Clone)]
pub struct Route(BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>);

impl Route {
    pub(super) fn new<T>(svc: T) -> Self
    where
        T: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>
            + Clone
            + Send
            + 'static,
        T::Future: Send + 'static,
    {
        Self(BoxCloneService::new(svc))
    }

    pub(crate) fn oneshot_inner(
        &self,
        req: Request<Bytes>,
    ) -> Oneshot<BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>, Request<Bytes>> {
        self.0.clone().oneshot(req)
    }
}

impl fmt::Debug for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route").finish()
    }
}

impl Service<Request<Bytes>> for Route {
    type Response = Response<Bytes>;
    type Error = Infallible;
    type Future =
        Oneshot<BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>, Request<Bytes>>;

    #[inline]
    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, req: Request<Bytes>) -> Self::Future {
        self.oneshot_inner(req)
    }
}
