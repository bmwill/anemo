use anemo::{Request, Response};
use bytes::Bytes;

pub use self::{
    future::ResponseFuture, layer::RequireAuthorizationLayer, service::RequireAuthorization,
};

mod future;
mod layer;
mod service;

/// Trait for authorizing requests.
pub trait AuthorizeRequest {
    /// Authorize the request.
    ///
    /// If `Ok(())` is returned then the request is allowed through, otherwise not.
    fn authorize(&self, request: &mut Request<Bytes>) -> Result<(), Response<Bytes>>;
}

impl<F> AuthorizeRequest for F
where
    F: Fn(&mut Request<Bytes>) -> Result<(), Response<Bytes>>,
{
    fn authorize(&self, request: &mut Request<Bytes>) -> Result<(), Response<Bytes>> {
        self(request)
    }
}

#[derive(Clone, Debug)]
pub struct AllowedPeers {
    allowed_peers: std::collections::HashSet<anemo::PeerId>,
}

impl AllowedPeers {
    pub fn new<P>(peers: P) -> Self
    where
        P: IntoIterator<Item = anemo::PeerId>,
    {
        Self {
            allowed_peers: peers.into_iter().collect(),
        }
    }
}

impl AuthorizeRequest for AllowedPeers {
    fn authorize(&self, request: &mut Request<Bytes>) -> Result<(), Response<Bytes>> {
        use anemo::types::response::{IntoResponse, StatusCode};

        let peer_id = request
            .peer_id()
            .ok_or_else(|| StatusCode::InternalServerError.into_response())?;

        if self.allowed_peers.contains(peer_id) {
            Ok(())
        } else {
            Err(StatusCode::NotFound.into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anemo::{
        types::response::{IntoResponse, StatusCode},
        PeerId, Request, Response,
    };
    use bytes::Bytes;
    use tower::{BoxError, Service, ServiceBuilder, ServiceExt};

    #[tokio::test]
    async fn authorize_request_fn() {
        const AUTH_HEADER: &str = "authorize";

        // Authorize requests that have a particular header set
        let auth_layer = RequireAuthorizationLayer::new(|request: &mut Request<Bytes>| {
            if request.headers().contains_key(AUTH_HEADER) {
                Ok(())
            } else {
                Err(StatusCode::NotFound.into_response())
            }
        });

        let mut svc = ServiceBuilder::new().layer(auth_layer).service_fn(echo);

        // Unauthorized Request
        let response = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NotFound);

        // Authorized Request
        let response = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")).with_header(AUTH_HEADER, "0"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.inner(), "foobar");
    }

    #[tokio::test]
    async fn authorize_request_by_peer_id() {
        let allowed_peer_1 = PeerId([42; 32]);
        let allowed_peer_2 = PeerId([13; 32]);
        let disallowed_peer = PeerId([9; 32]);

        // Authorize requests that have a particular header set
        let auth_layer =
            RequireAuthorizationLayer::new(AllowedPeers::new([allowed_peer_1, allowed_peer_2]));

        let mut svc = ServiceBuilder::new().layer(auth_layer).service_fn(echo);

        // Unable to query requester's PeerId
        let response = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::InternalServerError);

        // Unauthorized Request
        let response = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")).with_extension(disallowed_peer))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NotFound);

        // Authorized Request
        let response = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")).with_extension(allowed_peer_1))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.inner(), "foobar");

        // Authorized Request
        let response = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("bar")).with_extension(allowed_peer_2))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.inner(), "bar");
    }

    async fn echo(req: Request<Bytes>) -> Result<Response<Bytes>, BoxError> {
        Ok(Response::new(req.into_body()))
    }
}
