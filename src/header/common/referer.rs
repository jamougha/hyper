use header::{Header, HeaderFormat};
use std::str::{from_utf8, FromStr};
use std::fmt::{mod, Show, Formatter, Error};
use url::format::{PathFormatter};
use url::SchemeData::{NonRelative, Relative};
use Url;

/// The "Referer" [sic] header field allows the user agent to specify a
/// URI reference for the resource from which the target URI was obtained
/// i.e., the "referrer".
/// See also https://tools.ietf.org/html/rfc7231#section-5.5.2

#[deriving(Clone, PartialEq, Show)]
pub enum Referer {
    /// A referer header containing a URI.
    RefererUrl(Url),
    /// A referer header containing the text 'about:blank'.
    Blank,
}

impl Header for Referer {

    fn header_name(_: Option<Referer>) -> &'static str {
        "Referer"
    }

    fn parse_header(raw: &[Vec<u8>]) -> Option<Referer> {
        if raw.len() != 1 {
            None
        } else {
            from_utf8(raw[0].as_slice())
                .map(|h| h.trim())
                .and_then(|s| FromStr::from_str(s))
        }
    }

}

impl FromStr for Referer {
    fn from_str(s: &str) -> Option<Referer> {
        if s == "about:blank" {
            Some(Referer::Blank)
        } else {
            Url::parse(s).ok()
                .map(|url| Referer::RefererUrl(url))
        }

    }
}

impl HeaderFormat for Referer {

    fn fmt_header(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Referer::Blank => "about:blank".fmt(fmt),
            // Don't use the standard URL formatter because
            // "A user agent MUST NOT include the fragment and
            // userinfo components of the URI reference"
            // https://tools.ietf.org/html/rfc7231#section-5.5.2
            Referer::RefererUrl(ref url) => {
                try!(fmt.write(url.scheme.as_bytes()));
                try!(fmt.write(b":"));

                match url.scheme_data {
                    NonRelative(_) => try!(url.scheme_data.fmt(fmt)),
                    Relative(ref data) => {
                        try!(fmt.write(b"//"));

                        try!(data.host.fmt(fmt));

                        if let Some(port) = data.port {
                            try!(write!(fmt, ":{}", port));
                        }

                        try!(PathFormatter {
                            path: data.path.as_slice()
                        }.fmt(fmt));
                    }
                }

                if let Some(ref query) = url.query {
                    try!(fmt.write(b"?"));
                    try!(fmt.write(query.as_bytes()));
                }

                Ok(())
            }
        } 

    }
}


// This is just testing boilerplate.
struct RefererFormatter<'a>(pub &'a Referer);

impl<'a> Show for RefererFormatter<'a> {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), Error> {
        let RefererFormatter(referer) = *self;
        referer.fmt_header(fmt)
    }
}

#[test]
fn test_parse_absolute_uri() {
    let header: Vec<u8> = "http://www.example.com/foo/bar?baz#fragment"
                            .as_bytes().iter().map(|x| *x).collect();
    let referer: Referer = Header::parse_header(&[header]).unwrap();

    if let Referer::RefererUrl(ref url) = referer {

        assert_eq!("http", url.scheme);
        let url_string = "//www.example.com/foo/bar";
        assert_eq!(url_string, format!("{}", url.scheme_data));
        assert_eq!(Some("baz".to_string()), url.query);
        
        assert_eq!("http://www.example.com/foo/bar?baz", 
                    format!("{}", RefererFormatter(&referer)));
    } else {
        panic!();
    }
}

#[test]
fn test_remove_userinfo() {
    let header: Vec<u8> = "http://userinfo:foo@host:9/foo.bar"
                            .as_bytes().iter().map(|x| *x).collect();
    let referer: Referer = Header::parse_header(&[header]).unwrap();

    assert_eq!("http://host:9/foo.bar".to_string(), 
                format!("{}", RefererFormatter(&referer)));
}

#[test]
fn test_url() {
    let url = Url::parse("foo/bar");
    match url {
        Some(_) => println!("OK!"),
        None => panic!()
    }
}

#[test]
fn test_parse_relative_uri() {
    let header: Vec<u8> = "foo/bar?baz"
                            .as_bytes().iter().map(|x| *x).collect();
    let referer: Option<Referer> = Header::parse_header(&[header]);
    referer.expect("We should be using a URI here, not a URL.");
}