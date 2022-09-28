use super::ResponseHandler;
use anemo::Response;
use bytes::Bytes;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pin_project! {
    /// Response future for [`Callback`].
    ///
    /// [`Callback`]: super::Callback
    pub struct ResponseFuture<F, ResponseHandler> {
        #[pin]
        pub(crate) inner: F,
        pub(crate) handler: Option<ResponseHandler>,
    }
}

impl<Fut, E, ResponseHandlerT> Future for ResponseFuture<Fut, ResponseHandlerT>
where
    Fut: Future<Output = Result<Response<Bytes>, E>>,
    ResponseHandlerT: ResponseHandler,
{
    type Output = Result<Response<Bytes>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let result = futures::ready!(this.inner.poll(cx));
        let handler = this.handler.take().unwrap();

        match &result {
            Ok(response) => {
                handler.on_response(response);
            }
            Err(err) => {
                handler.on_error(err);
            }
        }

        Poll::Ready(result)
    }
}
