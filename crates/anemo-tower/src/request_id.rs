//! Set and propagate request ids.
//!
//! # Example
//!
//! ```
//! use anemo::{Request, Response};
//! use tower::{Service, ServiceExt, ServiceBuilder};
//! use anemo_tower::request_id::{
//!     SetRequestIdLayer, PropagateRequestIdLayer, MakeRequestId, RequestId,
//! };
//! use bytes::Bytes;
//! use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let handler = tower::service_fn(|request: Request<Bytes>| async move {
//! #     Ok::<_, std::convert::Infallible>(Response::new(request.into_body()))
//! # });
//! #
//! // A `MakeRequestId` that increments an atomic counter
//! #[derive(Clone, Default)]
//! struct MyMakeRequestId {
//!     counter: Arc<AtomicU64>,
//! }
//!
//! impl MakeRequestId for MyMakeRequestId {
//!     fn make_request_id<B>(&mut self, request: &Request<B>) -> Option<RequestId> {
//!         let request_id = self.counter
//!             .fetch_add(1, Ordering::SeqCst)
//!             .to_string()
//!             .parse()
//!             .unwrap();
//!
//!         Some(RequestId::new(request_id))
//!     }
//! }
//!
//! let request_id = "request-id".to_owned();
//!
//! let mut svc = ServiceBuilder::new()
//!     // set `request-id` header on all requests
//!     .layer(SetRequestIdLayer::new(
//!         request_id.clone(),
//!         MyMakeRequestId::default(),
//!     ))
//!     // propagate `request-id` headers from request to response
//!     .layer(PropagateRequestIdLayer::new(request_id))
//!     .service(handler);
//!
//! let request = Request::new(Bytes::new());
//! let response = svc.ready().await?.call(request).await?;
//!
//! assert_eq!(response.headers()["request-id"], "0");
//! #
//! # Ok(())
//! # }
//! ```
//!
//! # Using `Trace`
//!
//! To have request ids show up correctly in logs produced by [`Trace`] you must apply the layers
//! in this order:
//!
//! ```
//! use anemo_tower::{
//!     trace::{TraceLayer, DefaultMakeSpan, DefaultOnResponse},
//! };
//! # use anemo::{Request, Response};
//! # use tower::{Service, ServiceExt, ServiceBuilder};
//! # use anemo_tower::request_id::{
//! #     SetRequestIdLayer, PropagateRequestIdLayer, MakeRequestId, RequestId,
//! # };
//! # use bytes::Bytes;
//! # use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let handler = tower::service_fn(|request: Request<Bytes>| async move {
//! #     Ok::<_, std::convert::Infallible>(Response::new(request.into_body()))
//! # });
//! # #[derive(Clone, Default)]
//! # struct MyMakeRequestId {
//! #     counter: Arc<AtomicU64>,
//! # }
//! # impl MakeRequestId for MyMakeRequestId {
//! #     fn make_request_id<B>(&mut self, request: &Request<B>) -> Option<RequestId> {
//! #         let request_id = self.counter
//! #             .fetch_add(1, Ordering::SeqCst)
//! #             .to_string()
//! #             .parse()
//! #             .unwrap();
//! #         Some(RequestId::new(request_id))
//! #     }
//! # }
//!
//! let svc = ServiceBuilder::new()
//!     // make sure to set request ids before the request reaches `TraceLayer`
//!     .layer(SetRequestIdLayer::request_id(MyMakeRequestId::default()))
//!     // log requests and responses
//!     .layer(
//!         TraceLayer::new_for_server_errors()
//!             .make_span_with(DefaultMakeSpan::new().include_headers(true))
//!             .on_response(DefaultOnResponse::new().include_headers(true))
//!     )
//!     // propagate the header to the response before the response reaches `TraceLayer`
//!     .layer(PropagateRequestIdLayer::request_id())
//!     .service(handler);
//! #
//! # Ok(())
//! # }
//! ```
//!
//! # Doesn't override existing headers
//!
//! [`SetRequestId`] and [`PropagateRequestId`] wont override request ids if its already present on
//! requests or responses. Among other things, this allows other middleware to conditionally set
//! request ids and use the middleware in this module as a fallback.
//!
//! [`Uuid`]: https://crates.io/crates/uuid
//! [`Trace`]: crate::trace::Trace

use anemo::{Request, Response};
use pin_project_lite::pin_project;
use std::task::{Context, Poll};
use std::{future::Future, pin::Pin};
use tower::Layer;
use tower::Service;
use uuid::Uuid;

pub(crate) const REQUEST_ID: &str = "request-id";

/// Trait for producing [`RequestId`]s.
///
/// Used by [`SetRequestId`].
pub trait MakeRequestId {
    /// Try and produce a [`RequestId`] from the request.
    fn make_request_id<B>(&mut self, request: &Request<B>) -> Option<RequestId>;
}

/// An identifier for a request.
#[derive(Debug, Clone)]
pub struct RequestId(String);

impl RequestId {
    /// Create a new `RequestId` from a header value.
    pub fn new(header_value: String) -> Self {
        Self(header_value)
    }

    /// Gets a reference to the underlying header value.
    pub fn inner(&self) -> &str {
        &self.0
    }

    /// Consumes `self`, returning the underlying header value.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for RequestId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Set request id headers and extensions on requests.
///
/// This layer applies the [`SetRequestId`] middleware.
///
/// See the [module docs](self) and [`SetRequestId`] for more details.
#[derive(Debug, Clone)]
pub struct SetRequestIdLayer<M> {
    header_name: String,
    make_request_id: M,
}

impl<M> SetRequestIdLayer<M> {
    /// Create a new `SetRequestIdLayer`.
    pub fn new(header_name: String, make_request_id: M) -> Self
    where
        M: MakeRequestId,
    {
        SetRequestIdLayer {
            header_name,
            make_request_id,
        }
    }

    /// Create a new `SetRequestIdLayer` that uses `request-id` as the header name.
    pub fn request_id(make_request_id: M) -> Self
    where
        M: MakeRequestId,
    {
        SetRequestIdLayer::new(REQUEST_ID.to_owned(), make_request_id)
    }
}

impl<S, M> Layer<S> for SetRequestIdLayer<M>
where
    M: Clone + MakeRequestId,
{
    type Service = SetRequestId<S, M>;

    fn layer(&self, inner: S) -> Self::Service {
        SetRequestId::new(
            inner,
            self.header_name.clone(),
            self.make_request_id.clone(),
        )
    }
}

/// Set request id headers and extensions on requests.
///
/// See the [module docs](self) for an example.
///
/// If [`MakeRequestId::make_request_id`] returns `Some(_)` and the request doesn't already have a
/// header with the same name, then the header will be inserted.
///
/// Additionally [`RequestId`] will be inserted into [`Request::extensions`] so other
/// services can access it.
#[derive(Debug, Clone)]
pub struct SetRequestId<S, M> {
    inner: S,
    header_name: String,
    make_request_id: M,
}

impl<S, M> SetRequestId<S, M> {
    /// Create a new `SetRequestId`.
    pub fn new(inner: S, header_name: String, make_request_id: M) -> Self
    where
        M: MakeRequestId,
    {
        Self {
            inner,
            header_name,
            make_request_id,
        }
    }

    /// Create a new `SetRequestId` that uses `request-id` as the header name.
    pub fn request_id(inner: S, make_request_id: M) -> Self
    where
        M: MakeRequestId,
    {
        Self::new(inner, REQUEST_ID.to_owned(), make_request_id)
    }

    /// Returns a new [`Layer`] that wraps services with a `SetRequestId` middleware.
    pub fn layer(header_name: String, make_request_id: M) -> SetRequestIdLayer<M>
    where
        M: MakeRequestId,
    {
        SetRequestIdLayer::new(header_name, make_request_id)
    }

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

impl<S, M, ReqBody, ResBody> Service<Request<ReqBody>> for SetRequestId<S, M>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    M: MakeRequestId,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        if let Some(request_id) = req.headers().get(&self.header_name) {
            if req.extensions().get::<RequestId>().is_none() {
                let request_id = request_id.clone();
                req.extensions_mut().insert(RequestId::new(request_id));
            }
        } else if let Some(request_id) = self.make_request_id.make_request_id(&req) {
            req.extensions_mut().insert(request_id.clone());
            req.headers_mut()
                .insert(self.header_name.clone(), request_id.0);
        }

        self.inner.call(req)
    }
}

/// Propagate request ids from requests to responses.
///
/// This layer applies the [`PropagateRequestId`] middleware.
///
/// See the [module docs](self) and [`PropagateRequestId`] for more details.
#[derive(Debug, Clone)]
pub struct PropagateRequestIdLayer {
    header_name: String,
}

impl PropagateRequestIdLayer {
    /// Create a new `PropagateRequestIdLayer`.
    pub fn new(header_name: String) -> Self {
        PropagateRequestIdLayer { header_name }
    }

    /// Create a new `PropagateRequestIdLayer` that uses `request-id` as the header name.
    pub fn request_id() -> Self {
        Self::new(REQUEST_ID.to_owned())
    }
}

impl<S> Layer<S> for PropagateRequestIdLayer {
    type Service = PropagateRequestId<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PropagateRequestId::new(inner, self.header_name.clone())
    }
}

/// Propagate request ids from requests to responses.
///
/// See the [module docs](self) for an example.
///
/// If the request contains a matching header that header will be applied to responses. If a
/// [`RequestId`] extension is also present it will be propagated as well.
#[derive(Debug, Clone)]
pub struct PropagateRequestId<S> {
    inner: S,
    header_name: String,
}

impl<S> PropagateRequestId<S> {
    /// Create a new `PropagateRequestId`.
    pub fn new(inner: S, header_name: String) -> Self {
        Self { inner, header_name }
    }

    /// Create a new `PropagateRequestId` that uses `request-id` as the header name.
    pub fn request_id(inner: S) -> Self {
        Self::new(inner, REQUEST_ID.to_owned())
    }

    /// Returns a new [`Layer`] that wraps services with a `PropagateRequestId` middleware.
    pub fn layer(header_name: String) -> PropagateRequestIdLayer {
        PropagateRequestIdLayer::new(header_name)
    }

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

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for PropagateRequestId<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = PropagateRequestIdResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let request_id = req
            .headers()
            .get(&self.header_name)
            .cloned()
            .map(RequestId::new);

        PropagateRequestIdResponseFuture {
            inner: self.inner.call(req),
            header_name: self.header_name.clone(),
            request_id,
        }
    }
}

pin_project! {
    /// Response future for [`PropagateRequestId`].
    pub struct PropagateRequestIdResponseFuture<F> {
        #[pin]
        inner: F,
        header_name: String,
        request_id: Option<RequestId>,
    }
}

impl<F, B, E> Future for PropagateRequestIdResponseFuture<F>
where
    F: Future<Output = Result<Response<B>, E>>,
{
    type Output = Result<Response<B>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let mut response = futures::ready!(this.inner.poll(cx))?;

        if let Some(current_id) = response.headers().get(&*this.header_name) {
            if response.extensions().get::<RequestId>().is_none() {
                let current_id = current_id.clone();
                response.extensions_mut().insert(RequestId::new(current_id));
            }
        } else if let Some(request_id) = this.request_id.take() {
            response
                .headers_mut()
                .insert(this.header_name.clone(), request_id.0.clone());
            response.extensions_mut().insert(request_id);
        }

        Poll::Ready(Ok(response))
    }
}

/// A [`MakeRequestId`] that generates `UUID`s.
#[derive(Clone, Copy)]
pub struct MakeRequestUuid;

impl MakeRequestId for MakeRequestUuid {
    fn make_request_id<B>(&mut self, _request: &Request<B>) -> Option<RequestId> {
        let request_id = Uuid::new_v4().to_string();
        Some(RequestId::new(request_id))
    }
}

#[cfg(test)]
mod tests {
    use anemo::Request;
    use anemo::Response;
    use bytes::Bytes;
    use std::{
        convert::Infallible,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
    };
    use tower::{ServiceBuilder, ServiceExt};

    #[allow(unused_imports)]
    use super::*;

    #[tokio::test]
    async fn basic() {
        let svc = ServiceBuilder::new()
            .layer(SetRequestIdLayer::request_id(Counter::default()))
            .layer(PropagateRequestIdLayer::request_id())
            .service_fn(handler);

        // header on response
        let req = Request::new(Bytes::new());
        let res = svc.clone().oneshot(req).await.unwrap();
        assert_eq!(res.headers()["request-id"], "0");

        let req = Request::new(Bytes::new());
        let res = svc.clone().oneshot(req).await.unwrap();
        assert_eq!(res.headers()["request-id"], "1");

        // doesn't override if header is already there
        let req = Request::new(Bytes::new()).with_header("request-id", "foo");
        let res = svc.clone().oneshot(req).await.unwrap();
        assert_eq!(res.headers()["request-id"], "foo");

        // extension propagated
        let req = Request::new(Bytes::new());
        let res = svc.clone().oneshot(req).await.unwrap();
        assert_eq!(res.extensions().get::<RequestId>().unwrap().0, "2");
    }

    #[derive(Clone, Default)]
    struct Counter(Arc<AtomicU64>);

    impl MakeRequestId for Counter {
        fn make_request_id<B>(&mut self, _request: &Request<B>) -> Option<RequestId> {
            let id = self.0.fetch_add(1, Ordering::SeqCst).to_string();
            Some(RequestId::new(id))
        }
    }

    async fn handler(_: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
        Ok(Response::new(Bytes::new()))
    }

    #[tokio::test]
    async fn uuid() {
        let svc = ServiceBuilder::new()
            .layer(SetRequestIdLayer::request_id(MakeRequestUuid))
            .layer(PropagateRequestIdLayer::request_id())
            .service_fn(handler);

        // header on response
        let req = Request::new(Bytes::new());
        let mut res = svc.clone().oneshot(req).await.unwrap();
        let id = res.headers_mut().remove("request-id").unwrap();
        id.parse::<Uuid>().unwrap();
    }
}
