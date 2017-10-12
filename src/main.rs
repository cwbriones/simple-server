extern crate flate2;
extern crate futures;
extern crate futures_cpupool;
extern crate hyper;
extern crate mime;

#[macro_use]
extern crate log;
extern crate pretty_env_logger;
extern crate time;

use futures::{Async, Future, Poll};
use futures_cpupool::Builder as PoolBuilder;
use futures_cpupool::{CpuFuture, CpuPool};
use hyper::{Method, Request, Response, StatusCode};
use hyper::server::Http;
use hyper::server::Service;
use hyper::header::{AcceptEncoding, ContentEncoding, ContentLength, ContentType, Encoding};

use std::fs::File;
use std::path::{Path, PathBuf};
use std::io::{BufReader, Read};

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
        ResponseFuture::Found(self.pool.spawn_fn(move || read_file(&canonical, gzip)))
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

fn read_file(canonical: &Path, accept_gzip: bool) -> Result<Response, Error> {
    debug!("==> {:?}", canonical);
    let file = File::open(canonical)?;
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
    if let Some(c) = content_type(canonical) {
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
        _ => ext.parse().ok().map(ContentType),
    }
}

struct RequestLogger(Request, ResponseFuture, u64);

impl RequestLogger {
    fn log(&self, response: &Response) {
        let duration_us = (time::precise_time_ns() - self.2) / 1_000;

        let req = &self.0;
        let status = response.status().as_u16();
        debug!("[{}] {} {} \t{}µs", status, req.method(), req.path(), duration_us);
    }
}

impl Future for RequestLogger {
    type Item = Response;
    type Error = ::hyper::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let inner = self.1.poll();
        if let Ok(Async::Ready(ref res)) = inner {
            self.log(res);
        }
        inner
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
        inner
            .poll()
            .or_else(|e| translate_error(e).map(Async::Ready))
    }
}

fn translate_error(err: Error) -> Result<Response, ::hyper::Error> {
    match err {
        Error::Hyper(e) => Err(e),
        Error::FileNotFound => Ok(Response::new().with_status(StatusCode::NotFound)),
        e => {
            error!("{}", e);
            Ok(Response::new().with_status(StatusCode::InternalServerError))
        }
    }
}

impl Service for StaticServer {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = RequestLogger;

    fn call(&self, req: Request) -> Self::Future {
        let req_start = time::precise_time_ns();
        if *req.method() != Method::Get {
            return RequestLogger(req, ResponseFuture::NotAllowed, req_start);
        }
        let path = {
            // Strip the leading '/' since PathBuf will overwrite
            PathBuf::from(&req.path()[1..])
        };
        let gzip = req.headers()
            .get::<AcceptEncoding>()
            .map(|es| es.iter().any(|q| q.item == Encoding::Gzip))
            .unwrap_or(false);

        RequestLogger(req, self.spawn_read(&path, gzip), req_start)
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
            .unwrap_or_else(|| "./public".into());

        let port = args.next()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8080);

        Params { root, port }
    }
}

fn main() {
    pretty_env_logger::init().unwrap();
    let Params { root, port } = Params::parse();
    let pool = PoolBuilder::new()
        .pool_size(4)
        .name_prefix("fs-thread")
        .create();

    let addr = ([127, 0, 0, 1], port).into();
    info!("Serving {:?} at http://{}", &root, addr);

    let service = StaticServer {
        root: root,
        pool: pool,
    };
    let server = Http::new()
        .bind(&addr, move || Ok(service.clone()))
        .unwrap();

    server.run().unwrap();
}
