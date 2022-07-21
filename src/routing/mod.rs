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
#[derive(Clone)]
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::response::{IntoResponse, StatusCode};
    use tower::{util::BoxCloneService, ServiceExt};
    use tracing::trace;

    #[test]
    fn router_add_simple_route() {
        let router = Router::new();
        router.route("/echo", echo_service());
    }

    #[test]
    #[should_panic(expected = "Paths must start with a `/`.")]
    fn router_empty_path() {
        let router = Router::new();
        router.route("", echo_service());
    }

    #[test]
    #[should_panic(expected = "Paths must start with a `/`")]
    fn router_path_does_not_start_with_slash() {
        let router = Router::new();
        router.route("", echo_service());
    }

    #[test]
    #[should_panic(expected = "Invalid route: `Router::route` cannot be used with `Router`s.")]
    fn router_cant_use_router_as_route_service() {
        let router = Router::new();
        router.route("/nested", Router::new());
    }

    #[tokio::test]
    async fn router_with_multiple_routes() {
        let hello_world = tower::service_fn(|_request| async {
            Ok(Response::new(Bytes::from_static(b"hello world!")))
        });

        let internal_server_error = tower::service_fn(|_request| async {
            Ok(StatusCode::InternalServerError.into_response())
        });

        let router = Router::new()
            .route("/echo", echo_service())
            .route("/hello-world", hello_world)
            .route("/internal/server/error", internal_server_error);

        // Not Found Route
        let request = Request::new(Bytes::new());
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NotFound);

        // Echo
        let msg = b"echo this text";
        let mut request = Request::new(Bytes::from_static(msg));
        *request.route_mut() = "/echo".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), msg.as_ref());

        // Hello World
        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/hello-world".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");

        // Internal server error
        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/internal/server/error".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::InternalServerError);
    }

    #[tokio::test]
    async fn route_with_wildcard() {
        let hello_world_msg = b"hello world!";
        let hello_world = tower::service_fn(|_request| async {
            Ok(Response::new(Bytes::from_static(hello_world_msg)))
        });

        let router = Router::new().route("/hello/*rest", hello_world);

        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/hello/".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");

        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/hello/world".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");
    }

    fn echo_service() -> BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible> {
        let handle = move |request: Request<Bytes>| async move {
            trace!("recieved: {}", request.body().escape_ascii());
            let response = Response::new(request.into_body());
            Result::<Response<Bytes>, Infallible>::Ok(response)
        };

        tower::service_fn(handle).boxed_clone()
    }
}

// TODO
// In the future we should add support for applications to leaverage QUIC's capability of having
// unidirectional streams by having the "service" that is provided implement for Request as well as
// for Message input types
//
// pub struct Message<T> {
//     inner: Request<T>,
// }
//
// This is an example of a trait bound we could use
// fn route<T>(service: T)
// where
//     T: Clone + Send + 'static,
//     T: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>,
//     <T as Service<Request<Bytes>>>::Future: Send + 'static,
//     T: Service<Message<Bytes>, Response = (), Error = Infallible>,
//     <T as Service<Message<Bytes>>>::Future: Send + 'static,
// {
//     todo!()
// }
