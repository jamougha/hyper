//! HTTP Client
//!
//! # Usage
//!
//! The `Client` API is designed for most people to make HTTP requests.
//! It utilizes the lower level `Request` API.
//!
//! ```no_run
//! use hyper::Client;
//!
//! let mut client = Client::new();
//!
//! let mut res = client.get("http://example.domain").unwrap();
//! assert_eq!(res.status, hyper::Ok);
//! ```
//!
//! The returned value from is a `Response`, which provides easy access
//! to the `status`, the `headers`, and the response body via the `Writer`
//! trait.
use std::default::Default;
use std::io::{IoResult, BufReader};
use std::io::util::copy;
use std::iter::Extend;

use url::UrlParser;
use url::ParseError as UrlError;

use openssl::ssl::VerifyCallback;

use header::Headers;
use header::common::{ContentLength, Location};
use method::Method;
use net::{NetworkConnector, NetworkStream, HttpConnector};
use status::StatusClass::Redirection;
use {Url, Port, HttpResult};
use HttpError::HttpUriError;

pub use self::request::Request;
pub use self::response::Response;

pub mod request;
pub mod response;

/// A Client to use additional features with Requests.
///
/// Clients can handle things such as: redirect policy.
pub struct Client<C> {
    connector: C,
    redirect_policy: RedirectPolicy,
}

impl Client<HttpConnector> {

    /// Create a new Client.
    pub fn new() -> Client<HttpConnector> {
        Client::with_connector(HttpConnector(None))
    }

    /// Set the SSL verifier callback for use with OpenSSL.
    pub fn set_ssl_verifier(&mut self, verifier: VerifyCallback) {
        self.connector = HttpConnector(Some(verifier));
    }

}

impl<C: NetworkConnector<S>, S: NetworkStream> Client<C> {

    /// Create a new client with a specific connector.
    pub fn with_connector(connector: C) -> Client<C> {
        Client {
            connector: connector,
            redirect_policy: Default::default()
        }
    }

    /// Set the RedirectPolicy.
    pub fn set_redirect_policy(&mut self, policy: RedirectPolicy) {
        self.redirect_policy = policy;
    }

    /// Execute a Get request.
    pub fn get<U: IntoUrl>(&mut self, url: U) -> HttpResult<Response> {
        self.request(RequestOptions {
            method: Method::Get,
            url: url,
            headers: None,
            body: None::<&str>
        })
    }

    /// Execute a Head request.
    pub fn head<U: IntoUrl>(&mut self, url: U) -> HttpResult<Response> {
        self.request(RequestOptions {
            method: Method::Head,
            url: url,
            headers: None,
            body: None::<&str>
        })
    }

    /// Execute a Post request.
    pub fn post<'b, B: IntoBody<'b>, U: IntoUrl>(&mut self, url: U, body: B) -> HttpResult<Response> {
        self.request(RequestOptions {
            method: Method::Post,
            url: url,
            headers: None,
            body: Some(body),
        })
    }

    /// Execute a Put request.
    pub fn put<'b, B: IntoBody<'b>, U: IntoUrl>(&mut self, url: U, body: B) -> HttpResult<Response> {
        self.request(RequestOptions {
            method: Method::Put,
            url: url,
            headers: None,
            body: Some(body),
        })
    }

    /// Execute a Delete request.
    pub fn delete<'b, B: IntoBody<'b>, U: IntoUrl>(&mut self, url: U, body: B) -> HttpResult<Response> {
        self.request(RequestOptions {
            method: Method::Delete,
            url: url,
            headers: None,
            body: Some(body),
        })
    }


    /// Execute a request using this Client.
    pub fn request<'b, B: IntoBody<'b>, U: IntoUrl>(&mut self, options: RequestOptions<B, U>) -> HttpResult<Response> {
        // self is &mut because in the future, this function will check
        // self.connection_pool, inserting if empty, when keep_alive = true.

        let RequestOptions { method, url, headers, body } = options;
        let mut url = try!(url.into_url());
        debug!("client.request {} {}", method, url);

        let can_have_body = match &method {
            &Method::Get | &Method::Head => false,
            _ => true
        };

        let mut body = if can_have_body {
            body.map(|b| b.into_body())
        } else {
             None
        };

        loop {
            let mut req = try!(Request::with_connector(method.clone(), url.clone(), &mut self.connector));
            headers.as_ref().map(|headers| req.headers_mut().extend(headers.iter()));

            match (can_have_body, body.as_ref()) {
                (true, Some(ref body)) => match body.size() {
                    Some(size) => req.headers_mut().set(ContentLength(size)),
                    None => (), // chunked, Request will add it automatically
                },
                (true, None) => req.headers_mut().set(ContentLength(0)),
                _ => () // neither
            }
            let mut streaming = try!(req.start());
            body.take().map(|mut rdr| copy(&mut rdr, &mut streaming));
            let res = try!(streaming.send());
            if res.status.class() != Redirection {
                return Ok(res)
            }
            debug!("redirect code {} for {}", res.status, url);

            let loc = {
                // punching borrowck here
                let loc = match res.headers.get::<Location>() {
                    Some(&Location(ref loc)) => {
                        Some(UrlParser::new().base_url(&url).parse(loc[]))
                    }
                    None => {
                        debug!("no Location header");
                        // could be 304 Not Modified?
                        None
                    }
                };
                match loc {
                    Some(r) => r,
                    None => return Ok(res)
                }
            };
            url = match loc {
                Ok(u) => {
                    inspect!("Location", u)
                },
                Err(e) => {
                    debug!("Location header had invalid URI: {}", e);
                    return Ok(res);
                }
            };
            match self.redirect_policy {
                // separate branches because they cant be one
                RedirectPolicy::FollowAll => (), //continue
                RedirectPolicy::FollowIf(cond) if cond(&url) => (), //continue
                _ => return Ok(res),
            }
        }
    }
}

/// Options for an individual Request.
///
/// One of these will be built for you if you use one of the convenience
/// methods, such as `get()`, `post()`, etc.
pub struct RequestOptions<'a, B: IntoBody<'a>, U: IntoUrl> {
    /// The url for this request.
    pub url: U,
    /// If any additional headers should be sent.
    pub headers: Option<Headers>,
    /// The Request Method, such as `Get`, `Post`, etc.
    pub method: Method,
    /// If a request body should be sent.
    pub body: Option<B>,
}

/// A helper trait to allow overloading of the body parameter.
pub trait IntoBody<'a> {
    /// Consumes self into an instance of `Body`.
    fn into_body(self) -> Body<'a>;
}

/// The target enum for the IntoBody trait.
pub enum Body<'a> {
    /// A Reader does not necessarily know it's size, so it is chunked.
    ChunkedBody(&'a mut (Reader + 'a)),
    /// A String has a size, and uses Content-Length.
    SizedBody(BufReader<'a>, uint),
}

impl<'a> Body<'a> {
    fn size(&self) -> Option<uint> {
        match *self {
            Body::SizedBody(_, len) => Some(len),
            _ => None
        }
    }
}

impl<'a> Reader for Body<'a> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> IoResult<uint> {
        match *self {
            Body::ChunkedBody(ref mut r) => r.read(buf),
            Body::SizedBody(ref mut r, _) => r.read(buf),
        }
    }
}

impl<'a> IntoBody<'a> for &'a [u8] {
    #[inline]
    fn into_body(self) -> Body<'a> {
        Body::SizedBody(BufReader::new(self), self.len())
    }
}

impl<'a> IntoBody<'a> for &'a str {
    #[inline]
    fn into_body(self) -> Body<'a> {
        self.as_bytes().into_body()
    }
}

impl<'a, R: Reader> IntoBody<'a> for &'a mut R {
    #[inline]
    fn into_body(self) -> Body<'a> {
        Body::ChunkedBody(self)
    }
}

/// A helper trait to convert common objects into a Url.
pub trait IntoUrl {
    /// Consumes the object, trying to return a Url.
    fn into_url(self) -> Result<Url, UrlError>;
}

impl IntoUrl for Url {
    fn into_url(self) -> Result<Url, UrlError> {
        Ok(self)
    }
}

impl<'a> IntoUrl for &'a str {
    fn into_url(self) -> Result<Url, UrlError> {
        Url::parse(self)
    }
}

/// Behavior regarding how to handle redirects within a Client.
pub enum RedirectPolicy {
    /// Don't follow any redirects.
    FollowNone,
    /// Follow all redirects.
    FollowAll,
    /// Follow a redirect if the contained function returns true.
    FollowIf(fn(&Url) -> bool),
}

impl Default for RedirectPolicy {
    fn default() -> RedirectPolicy {
        RedirectPolicy::FollowAll
    }
}

fn get_host_and_port(url: &Url) -> HttpResult<(String, Port)> {
    let host = match url.serialize_host() {
        Some(host) => host,
        None => return Err(HttpUriError(UrlError::EmptyHost))
    };
    debug!("host={}", host);
    let port = match url.port_or_default() {
        Some(port) => port,
        None => return Err(HttpUriError(UrlError::InvalidPort))
    };
    debug!("port={}", port);
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use header::common::Server;
    use super::{Client, RedirectPolicy};
    use url::Url;

    mock_connector!(MockRedirectPolicy {
        "http://127.0.0.1" =>       "HTTP/1.1 301 Redirect\r\n\
                                     Location: http://127.0.0.2\r\n\
                                     Server: mock1\r\n\
                                     \r\n\
                                    "
        "http://127.0.0.2" =>       "HTTP/1.1 302 Found\r\n\
                                     Location: https://127.0.0.3\r\n\
                                     Server: mock2\r\n\
                                     \r\n\
                                    "
        "https://127.0.0.3" =>      "HTTP/1.1 200 OK\r\n\
                                     Server: mock3\r\n\
                                     \r\n\
                                    "
    })

    #[test]
    fn test_redirect_followall() {
        let mut client = Client::with_connector(MockRedirectPolicy);
        client.set_redirect_policy(RedirectPolicy::FollowAll);

        let res = client.get(Url::parse("http://127.0.0.1").unwrap()).unwrap();
        assert_eq!(res.headers.get(), Some(&Server("mock3".into_string())));
    }

    #[test]
    fn test_redirect_dontfollow() {
        let mut client = Client::with_connector(MockRedirectPolicy);
        client.set_redirect_policy(RedirectPolicy::FollowNone);
        let res = client.get(Url::parse("http://127.0.0.1").unwrap()).unwrap();
        assert_eq!(res.headers.get(), Some(&Server("mock1".into_string())));
    }

    #[test]
    fn test_redirect_followif() {
        fn follow_if(url: &Url) -> bool {
            !url.serialize()[].contains("127.0.0.3")
        }
        let mut client = Client::with_connector(MockRedirectPolicy);
        client.set_redirect_policy(RedirectPolicy::FollowIf(follow_if));
        let res = client.get(Url::parse("http://127.0.0.1").unwrap()).unwrap();
        assert_eq!(res.headers.get(), Some(&Server("mock2".into_string())));
    }

}
