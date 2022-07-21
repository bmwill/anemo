use crate::{wire::Version, HeaderMap, Result};

#[derive(Default)]
pub struct ResponseHeader {
    pub status: StatusCode,

    /// The response's version
    pub version: Version,

    pub headers: HeaderMap,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RawResponseHeader {
    pub status: u16,

    pub headers: HeaderMap,
}

impl RawResponseHeader {
    pub fn from_header(header: ResponseHeader) -> Self {
        Self {
            status: header.status.to_u16(),
            headers: header.headers,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum StatusCode {
    Success = 200,
    BadRequest = 400,
    NotFound = 404,
    InternalServerError = 500,
    VersionNotSupported = 505,
    Unknown = 520,
}

impl StatusCode {
    pub fn new(code: u16) -> Result<Self> {
        let status = match code {
            200 => Self::Success,
            400 => Self::BadRequest,
            500 => Self::InternalServerError,
            505 => Self::VersionNotSupported,
            _ => return Err(anyhow::anyhow!("invalid StatusCode {}", code)),
        };

        Ok(status)
    }

    pub fn to_u16(self) -> u16 {
        self as u16
    }
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

pub trait IntoResponse {
    /// Create a response.
    fn into_response(self) -> Response<bytes::Bytes>;
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response<bytes::Bytes> {
        let mut response = ().into_response();
        *response.status_mut() = self;
        response
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response<bytes::Bytes> {
        Response::new(bytes::Bytes::new())
    }
}
