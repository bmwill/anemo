use super::{AuthorizeRequest, RequireAuthorization};
use tower::Layer;

/// [`Layer`] that adds authorization to a [`Service`].
///
/// See the [module docs](crate::auth) for more details.
///
/// [`Layer`]: tower::layer::Layer
/// [`Service`]: tower::Service
#[derive(Debug, Copy, Clone)]
pub struct RequireAuthorizationLayer<A> {
    pub(super) auth: A,
}

impl<A> RequireAuthorizationLayer<A> {
    /// Create a new [`RequireAuthorizationLayer`] using the given [`AuthorizeRequest`].
    pub fn new(auth: A) -> Self
    where
        A: AuthorizeRequest,
    {
        Self { auth }
    }
}

impl<S, A> Layer<S> for RequireAuthorizationLayer<A>
where
    A: Clone,
{
    type Service = RequireAuthorization<S, A>;

    fn layer(&self, inner: S) -> Self::Service {
        RequireAuthorization {
            inner,
            auth: self.auth.clone(),
        }
    }
}
