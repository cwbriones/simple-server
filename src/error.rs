use std::error::Error as StdError;

#[derive(Debug)]
pub enum Error {
    Hyper(::hyper::Error),
    Io(::std::io::Error),
    Msg(String),
    FileNotFound,
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self {
            Error::Hyper(ref e) => e.description(),
            Error::Io(ref e) => e.description(),
            Error::FileNotFound => "The file was not found on the server",
            Error::Msg(ref msg) => msg,
        }
    }
}

impl ::std::fmt::Display for Error {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl From<::hyper::Error> for Error {
    fn from(error: ::hyper::Error) -> Error {
        Error::Hyper(error)
    }
}

impl From<::std::io::Error> for Error {
    fn from(error: ::std::io::Error) -> Error {
        use ::std::io::ErrorKind;

        if error.kind() == ErrorKind::NotFound {
            return Error::FileNotFound;
        }
        Error::Io(error)
    }
}

impl<'a> From<&'a str> for Error {
    fn from(s: &str) -> Error {
        Error::Msg(s.to_owned())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Error {
        Error::Msg(s)
    }
}

