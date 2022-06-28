use crate::{Request, Response};
use bytes::Bytes;
use std::{collections::HashMap, convert::Infallible};
use tower::{
    util::{BoxCloneService, Oneshot},
    Service,
};

mod not_found;
mod route;

use route::Route;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RouteId(u32);

impl RouteId {
    fn next() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        // `AtomicU64` isn't supported on all platforms
        static ID: AtomicU32 = AtomicU32::new(0);
        let id = ID.fetch_add(1, Ordering::Relaxed);
        if id == u32::MAX {
            panic!("Over `u32::MAX` routes created. If you need this, please file an issue.");
        }
        Self(id)
    }
}

/// The router type for composing handlers and services.
pub struct Router {
    routes: HashMap<RouteId, Route>,
    inner: matchit::Router<RouteId>,
    fallback: Route,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: Default::default(),
            inner: Default::default(),
            fallback: Route::new(not_found::NotFound),
        }
    }

    pub fn route<T>(mut self, path: &str, service: T) -> Self
    where
        T: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>
            + Clone
            + Send
            + 'static,
        T::Future: Send + 'static,
    {
        if path.is_empty() {
            panic!("Paths must start with a `/`. Use \"/\" for root routes");
        } else if !path.starts_with('/') {
            panic!("Paths must start with a `/`");
        }

        if <dyn std::any::Any>::downcast_ref::<Self>(&service).is_some() {
            panic!("Invalid route: `Router::route` cannot be used with `Router`s.")
        }

        let id = RouteId::next();

        let service = Route::new(service);

        if let Err(err) = self.inner.insert(path, id) {
            panic!("Invalid route: {}", err);
        }

        self.routes.insert(id, service);

        self
    }
}

impl Service<Request<Bytes>> for Router {
    type Response = Response<Bytes>;
    type Error = Infallible;
    type Future =
        Oneshot<BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>, Request<Bytes>>;

    #[inline]
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, req: Request<Bytes>) -> Self::Future {
        use matchit::MatchError;

        let path = req.route();

        match self.inner.at(path) {
            Ok(match_) => {
                let route = self
                    .routes
                    .get(match_.value)
                    .expect("no route for id; this is a bug");
                route.oneshot_inner(req)
            }
            Err(MatchError::MissingTrailingSlash)
            | Err(MatchError::ExtraTrailingSlash)
            | Err(MatchError::NotFound) => self.fallback.oneshot_inner(req),
        }
    }
}
