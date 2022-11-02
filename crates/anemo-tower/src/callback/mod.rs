use anemo::{Request, Response};
use bytes::Bytes;

pub use self::{future::ResponseFuture, layer::CallbackLayer, service::Callback};

mod future;
mod layer;
mod service;

pub trait MakeCallbackHandler {
    type Handler: ResponseHandler;

    fn make_handler(&self, request: &Request<Bytes>) -> Self::Handler;
}

pub trait ResponseHandler {
    fn on_response(self, response: &Response<Bytes>);
    fn on_error<E>(self, error: &E);
}

#[cfg(test)]
mod tests {
    use super::*;
    use anemo::{Request, Response};
    use bytes::Bytes;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };
    use tower::{BoxError, Service, ServiceBuilder, ServiceExt};

    #[derive(Clone)]
    struct MakeCallback {
        request_count: Arc<AtomicU32>,
        response_count: Arc<AtomicU32>,
        error_count: Arc<AtomicU32>,
        drop_count: Arc<AtomicU32>,
    }

    impl MakeCallbackHandler for MakeCallback {
        type Handler = Handler;

        fn make_handler(&self, _request: &Request<Bytes>) -> Self::Handler {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            Handler {
                response_count: self.response_count.clone(),
                error_count: self.error_count.clone(),
                drop_count: self.drop_count.clone(),
            }
        }
    }

    struct Handler {
        response_count: Arc<AtomicU32>,
        error_count: Arc<AtomicU32>,
        drop_count: Arc<AtomicU32>,
    }

    impl Drop for Handler {
        fn drop(&mut self) {
            self.drop_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl ResponseHandler for Handler {
        fn on_response(self, _response: &Response<Bytes>) {
            self.response_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_error<E>(self, _error: &E) {
            self.error_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn unary_request() {
        let request_count = Arc::new(AtomicU32::new(0));
        let response_count = Arc::new(AtomicU32::new(0));
        let error_count = Arc::new(AtomicU32::new(0));
        let drop_count = Arc::new(AtomicU32::new(0));

        let make_callback = MakeCallback {
            request_count: request_count.clone(),
            response_count: response_count.clone(),
            error_count: error_count.clone(),
            drop_count: drop_count.clone(),
        };

        let callback_layer = CallbackLayer::new(make_callback);

        let mut svc = ServiceBuilder::new().layer(callback_layer).service_fn(echo);

        let _res = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")))
            .await
            .unwrap();

        assert_eq!(1, request_count.load(Ordering::SeqCst), "request");
        assert_eq!(1, response_count.load(Ordering::SeqCst), "response");
        assert_eq!(0, error_count.load(Ordering::SeqCst), "error");
        assert_eq!(1, drop_count.load(Ordering::SeqCst), "drop");
    }

    async fn echo(req: Request<Bytes>) -> Result<Response<Bytes>, BoxError> {
        Ok(Response::new(req.into_body()))
    }
}
