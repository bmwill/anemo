use crate::{Request, Response};
use bytes::Bytes;
use std::convert::Infallible;
use tower::{
    util::{BoxCloneService, Oneshot},
    Service, ServiceExt,
};

#[derive(Clone)]
pub(super) struct Route(BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>);

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
