extern crate hyper;
extern crate futures;
extern crate mime;

use futures::future::{self, FutureResult};
use hyper::{Request, Response, Method, StatusCode};
use hyper::server::Http;
use hyper::server::Service;
use hyper::header::{ContentType, ContentLength};

use std::fs::File;
use std::path::{Path, PathBuf};
use std::io::{Read, BufReader};

use error::Error;
use std::env;

mod error;

#[derive(Clone)]
struct ServeStatic(PathBuf);

impl ServeStatic {
    fn read_and_respond(&self, path: &Path) -> Result<Response, ::hyper::Error> {
        let mut canonical = self.canonicalize(path);
        if canonical.is_dir() {
            canonical.push("index.html");
        }
        if canonical.extension().is_none() {
            canonical.set_extension("html");
        }
        let contents = read_file(&canonical);
        match contents {
            Ok(res) => Ok(res),
            Err(Error::Hyper(e)) => Err(e),
            Err(Error::FileNotFound) => Ok(Response::new().with_status(StatusCode::NotFound)),
            Err(ref e) => {
                println!("[ERROR]: {}", e);
                Ok(Response::new().with_status(StatusCode::InternalServerError))
            },
        }
    }

    fn canonicalize(&self, path: &Path) -> PathBuf {
        let mut canonical = PathBuf::from(&self.0);
        for component in path.components() {
            let c = component.as_ref();
            if c == ".." {
                canonical.pop();
            } else if c != "." {
                canonical.push(c)
            }
        }
        canonical
    }
}

fn read_file(canonical: &Path) -> Result<Response, Error> {
    println!("==> [DEBUG] {:?}", canonical);
    let file = File::open(canonical)?;
    let len = file.metadata()?.len();

    let mut file = BufReader::new(file);
    let mut body = Vec::with_capacity(len as usize);
    file.read_to_end(&mut body)?;

    let mut resp = Response::new()
        .with_body(body)
        .with_header(ContentLength(len));
    if let Some(c) = content_type(canonical) {
        resp = resp.with_header(c);
    }

    Ok(resp)
}

fn content_type(path: &Path) -> Option<ContentType> {
    let ext = match path.extension().and_then(|o| o.to_str()) {
        Some(ext) => ext,
        None => return None,
    };
    match ext {
        "jpg" | "jpeg" => Some(ContentType::jpeg()),
        "png" => Some(ContentType::png()),
        "txt" | "md" => Some(ContentType::plaintext()),
        "html" => Some(ContentType::html()),
        "xml" => Some(ContentType::xml()),
        "json" => Some(ContentType::json()),
        "gif" => "image/gif".parse().ok().map(ContentType),
        "css" => "text/css".parse().ok().map(ContentType),
        _ => ext.parse().ok().map(|m| ContentType(m)),
    }
}

impl Service for ServeStatic {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = FutureResult<Response, hyper::Error>;

    fn call(&self, req: Request) -> Self::Future {
        if req.method() != &Method::Get {
            let resp = Response::new().with_status(StatusCode::MethodNotAllowed);
            return future::result(Ok(resp));
        }
        let path = req.path();
        // Strip the leading '/' since PathBuf will overwrite
        let path = Path::new(&path[1..]);
        let result = self.read_and_respond(path);
        if let Ok(ref res) = result {
            let code = res.status().as_u16();
            println!("[{}] {} {}", code, req.method(), req.path());
        }
        future::result(result)
    }
}

struct Params {
    root: PathBuf,
    port: u16,
}

impl Params {
    fn parse() -> Self {
        let mut args = env::args();
        args.next();

        let root = args.next()
            .map(PathBuf::from)
            .unwrap_or(".".into());

        let port = args.next()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8080);

        Params {
            root,
            port,
        }
    }
}

fn main() {
    let params = Params::parse();
    println!("serving {:?} on port {}", &params.root, params.port);

    let addr = ([127, 0, 0, 1], params.port).into();
    let service = ServeStatic(params.root);
    let server = Http::new().bind(&addr, move || Ok(service.clone())).unwrap();

    server.run().unwrap();
}
