use anemo::Response;
use bytes::Bytes;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pin_project! {
    /// Response future for [`RequireAuthorization`].
    ///
    /// [`RequireAuthorization`]: super::RequireAuthorization
    pub struct ResponseFuture<F> {
        #[pin]
        kind: Kind<F>,
    }
}

impl<F> ResponseFuture<F> {
    pub(super) fn future(future: F) -> Self {
        Self {
            kind: Kind::Future { future },
        }
    }

    pub(super) fn invalid_auth(response: Response<Bytes>) -> Self {
        Self {
            kind: Kind::Error {
                response: Some(response),
            },
        }
    }
}

pin_project! {
    #[project = KindProj]
    enum Kind<F> {
        Future {
            #[pin]
            future: F,
        },
        Error {
            response: Option<Response<Bytes>>,
        },
    }
}

impl<F, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<Bytes>, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().kind.project() {
            KindProj::Future { future } => future.poll(cx),
            KindProj::Error { response } => {
                let response = response.take().unwrap();
                Poll::Ready(Ok(response))
            }
        }
    }
}
