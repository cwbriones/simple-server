extern crate hyper;
extern crate futures;
extern crate futures_cpupool;
extern crate mime;
extern crate flate2;

use futures::{Future, Poll, Async};
use futures_cpupool::Builder as PoolBuilder;
use futures_cpupool::{CpuPool, CpuFuture};
use hyper::{Request, Response, Method, StatusCode};
use hyper::server::Http;
use hyper::server::Service;
use hyper::header::{ContentType, ContentLength, ContentEncoding, Encoding, AcceptEncoding};

use std::fs::File;
use std::path::{Path, PathBuf};
use std::io::{Read, BufReader};

use error::Error;
use std::env;

mod error;

#[derive(Clone)]
struct StaticServer {
    root: PathBuf,
    pool: CpuPool,
}

impl StaticServer {
    fn spawn_read(&self, path: &Path, gzip: bool) -> ResponseFuture {
        let mut canonical = self.canonicalize(path);
        if canonical.is_dir() {
            canonical.push("index.html");
        }
        if canonical.extension().is_none() {
            canonical.set_extension("html");
        }
        ResponseFuture::Found(self.pool.spawn_fn(move || read_file(canonical, gzip)))
    }

    fn canonicalize(&self, path: &Path) -> PathBuf {
        let mut canonical = PathBuf::from(&self.root);
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

const MIN_GZIP_SIZE: u64 = 1024;

fn read_file(canonical: PathBuf, accept_gzip: bool) -> Result<Response, Error> {
    // println!("==> [DEBUG] {:?}", canonical);
    let file = File::open(&canonical)?;
    let len = file.metadata()?.len();

    let mut file = BufReader::new(file);
    let mut body = Vec::with_capacity(len as usize);

    let gzip = accept_gzip && len > MIN_GZIP_SIZE;

    if gzip {
        use flate2::Compression;
        use flate2::bufread::GzEncoder;

        let mut gz = GzEncoder::new(file, Compression::Fast);
        gz.read_to_end(&mut body)?;
    } else {
        file.read_to_end(&mut body)?;
    }

    let mut resp = Response::new()
        .with_body(body)
        .with_header(ContentLength(len));
    if let Some(c) = content_type(&canonical) {
        resp = resp.with_header(c);
    }
    if gzip {
        resp = resp.with_header(ContentEncoding(vec![Encoding::Gzip]));
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

enum ResponseFuture {
    Found(CpuFuture<Response, Error>),
    NotAllowed,
}

impl Future for ResponseFuture {
    type Item = Response;
    type Error = ::hyper::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let inner = match *self {
            ResponseFuture::Found(ref mut i) => i,
            ResponseFuture::NotAllowed => {
                let res = Response::new().with_status(StatusCode::MethodNotAllowed);
                return Ok(Async::Ready(res));
            }
        };
        inner.poll().or_else(|e| translate_error(e).map(Async::Ready))
    }
}

fn translate_error(err: Error) -> Result<Response, ::hyper::Error> {
    match err {
        Error::Hyper(e) => Err(e),
        Error::FileNotFound => Ok(Response::new().with_status(StatusCode::NotFound)),
        _ => {
            // println!("[ERROR]: {}", e);
            Ok(Response::new().with_status(StatusCode::InternalServerError))
        }
    }
}

impl Service for StaticServer {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = ResponseFuture;

    fn call(&self, req: Request) -> Self::Future {
        if *req.method() != Method::Get {
            return ResponseFuture::NotAllowed;
        }
        let path = req.path();
        // Strip the leading '/' since PathBuf will overwrite
        let path = Path::new(&path[1..]);
        let gzip = req.headers()
            .get::<AcceptEncoding>()
            .map(|es| es.iter().any(|q| q.item == Encoding::Gzip))
            .unwrap_or(false);

        self.spawn_read(path, gzip)
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
            .unwrap_or("./public".into());

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

    let pool = PoolBuilder::new()
        .pool_size(4)
        .name_prefix("fs-thread")
        .create();

    let addr = ([127, 0, 0, 1], params.port).into();
    let service = StaticServer {
        root: params.root,
        pool: pool,
    };
    let server = Http::new().bind(&addr, move || Ok(service.clone())).unwrap();

    server.run().unwrap();
}
