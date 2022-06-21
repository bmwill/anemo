use crate::{wire::Version, HeaderMap};

#[derive(Default)]
pub struct RequestHeader {
    pub route: String,

    /// The request's version
    pub version: Version,

    /// The request's headers
    pub headers: HeaderMap,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RawRequestHeader {
    pub route: String,

    pub headers: HeaderMap,
}

impl RawRequestHeader {
    pub fn from_header(header: RequestHeader) -> Self {
        Self {
            route: header.route,
            headers: header.headers,
        }
    }
}

pub struct Request<T> {
    head: RequestHeader,
    body: T,
}

impl<T> Request<T> {
    pub fn new(body: T) -> Request<T> {
        Self::from_parts(RequestHeader::default(), body)
    }

    pub fn from_parts(parts: RequestHeader, body: T) -> Request<T> {
        Self { head: parts, body }
    }

    pub fn route(&self) -> &str {
        &self.head.route
    }

    pub fn route_mut(&mut self) -> &mut String {
        &mut self.head.route
    }

    pub fn version(&self) -> Version {
        self.head.version
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.head.headers
    }

    pub fn body(&self) -> &T {
        &self.body
    }

    pub fn body_mut(&mut self) -> &mut T {
        &mut self.body
    }

    pub fn into_body(self) -> T {
        self.body
    }

    pub fn into_parts(self) -> (RequestHeader, T) {
        (self.head, self.body)
    }

    pub fn map<F, U>(self, f: F) -> Request<U>
    where
        F: FnOnce(T) -> U,
    {
        Request {
            body: f(self.body),
            head: self.head,
        }
    }
}
