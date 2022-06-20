use crate::{wire::Version, HeaderMap};

#[derive(Default)]
pub struct ResponseHeader {
    pub status: StatusCode,

    /// The response's version
    pub version: Version,

    pub headers: HeaderMap,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum StatusCode {
    Success = 200,
    BadRequest = 400,
    InternalServerError = 500,
    VersionNotSupported = 505,
}

impl Default for StatusCode {
    fn default() -> Self {
        Self::Success
    }
}

pub struct Response<T> {
    head: ResponseHeader,
    body: T,
}

impl<T> Response<T> {
    pub fn new(body: T) -> Response<T> {
        Self::from_parts(ResponseHeader::default(), body)
    }

    pub fn from_parts(parts: ResponseHeader, body: T) -> Response<T> {
        Self { head: parts, body }
    }

    pub fn status(&self) -> StatusCode {
        self.head.status
    }

    pub fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.head.status
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

    pub fn into_parts(self) -> (ResponseHeader, T) {
        (self.head, self.body)
    }

    pub fn map<F, U>(self, f: F) -> Response<U>
    where
        F: FnOnce(T) -> U,
    {
        Response {
            body: f(self.body),
            head: self.head,
        }
    }
}
