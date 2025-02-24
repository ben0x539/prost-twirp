use std::future::Future;
use std::future;
use std::task::{Poll, Context};
use std::pin::Pin;
use hyper;
use hyper::{body, header, Method, StatusCode, Uri, Version};
use hyper::body::Body;
use hyper::client::{Client, HttpConnector};
use hyper::service::Service;
use hyper::header::{HeaderMap, HeaderValue};
use prost::{DecodeError, EncodeError, Message};
use serde_json;
use std::sync::Arc;
use http::uri::InvalidUri;

use futures_util::{FutureExt, TryFutureExt};

type Request = hyper::Request<Body>;
type Response = hyper::Response<Body>;

pub type FutReq<T> = Pin<Box<dyn Future<Output=Result<ServiceRequest<T>, ProstTwirpError>>+Send>>;

/// The type of every service request 
pub type PTReq<I> = ServiceRequest<I>;

/// The type of every service response
pub type PTRes<O> = Pin<Box<dyn Future<Output=Result<ServiceResponse<O>, ProstTwirpError>>+Send>>;

/// A request with HTTP info and the serialized input object
#[derive(Debug)]
pub struct ServiceRequest<T> {
    /// The URI of the original request
    /// 
    /// When using a client, this will be overridden with the proper URI. It is only valuable for servers.
    pub uri: Uri,
    /// The request method; should always be Post
    pub method: Method,
    /// The HTTP version, rarely changed from the default
    pub version: Version,
    /// The set of headers
    ///
    /// Should always at least have `Content-Type`. Clients will override `Content-Length` on serialization.
    pub headers: HeaderMap,
    // The serialized request object
    pub input: T,
}

impl<T> ServiceRequest<T> {
    /// Create new service request with the given input object
    /// 
    /// This automatically sets the `Content-Type` header as `application/protobuf`.
    pub fn new(input: T) -> ServiceRequest<T> {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/protobuf"));
        ServiceRequest {
            uri: Default::default(),
            method: Method::POST,
            version: Version::default(),
            headers: headers,
            input
        }
    }
    
    /// Copy this request with a different input value
    pub fn clone_with_input<U>(&self, input: U) -> ServiceRequest<U> {
        ServiceRequest { uri: self.uri.clone(), method: self.method.clone(), version: self.version,
            headers: self.headers.clone(), input }
    }
}

impl<T: Message + Default + 'static> From<T> for ServiceRequest<T> {
    fn from(v: T) -> ServiceRequest<T> { ServiceRequest::new(v) }
}

impl ServiceRequest<Vec<u8>> {
    /// Turn a hyper request to a boxed future of a byte-array service request
    pub fn from_hyper_raw(req: Request) -> FutReq<Vec<u8>> {
        let uri = req.uri().clone();
        let method = req.method().clone();
        let version = req.version();
        let headers = req.headers().clone();
        Box::pin(body::to_bytes(req).map_err(ProstTwirpError::HyperError).map(move |result| result.map(move |body| {
            ServiceRequest { uri, method, version, headers, input: body.to_vec() }
        })))
    }

    /// Turn a byte-array service request into a hyper request
    pub fn to_hyper_raw(&self) -> Request {
        let mut req = hyper::Request::post(&self.uri)
            .body(self.input.clone().into())
            .unwrap(); // TODO dont unwrap
        req.headers_mut().clone_from(&self.headers);
        req.headers_mut().insert(header::CONTENT_LENGTH, self.input.len().into());
        req
    }

    /// Turn a byte-array service request into a `AfterBodyError`-wrapped version of the given error
    pub fn body_err(&self, err: ProstTwirpError) -> ProstTwirpError {
        ProstTwirpError::AfterBodyError {
            body: self.input.clone(), method: Some(self.method.clone()), version: self.version,
            headers: self.headers.clone(), status: None, err: Box::new(err)
        }
    }

    /// Serialize the byte-array service request into a protobuf service request
    pub fn to_proto<T: Message + Default + 'static>(&self) -> Result<ServiceRequest<T>, ProstTwirpError> {
        match T::decode(&*self.input) {
            Ok(v) => Ok(self.clone_with_input(v)),
            Err(err) => Err(self.body_err(ProstTwirpError::ProstDecodeError(err)))
        }
    }
}

impl<T: Message + Default + 'static> ServiceRequest<T> {
    /// Turn a protobuf service request into a byte-array service request
    pub fn to_proto_raw(&self) -> Result<ServiceRequest<Vec<u8>>, ProstTwirpError> {
        let mut body = Vec::new();
        if let Err(err) = self.input.encode(&mut body) {
            Err(ProstTwirpError::ProstEncodeError(err))
        } else {
            Ok(self.clone_with_input(body))
        }
    }

    /// Turn a hyper request into a protobuf service request
    pub fn from_hyper_proto(req: Request) -> FutReq<T> {
        // TODO: this used to be just Box::new(ServiceRequest::from_hyper_raw(req).and_then(|v| v.to_proto()))
        let uri = req.uri().clone();
        let method = req.method().clone();
        let version = req.version();
        let headers = req.headers().clone();
        Box::pin(body::to_bytes(req).map_err(ProstTwirpError::HyperError).map(move |result| result.and_then(move |body| {
            ServiceRequest { uri, method, version, headers, input: body.to_vec() }.to_proto()
        })))
    }

    /// Turn a protobuf service request into a hyper request
    pub fn to_hyper_proto(&self) -> Result<Request, ProstTwirpError> {
        self.to_proto_raw().map(|v| v.to_hyper_raw())
    }
}

/// A response with HTTP info and a serialized output object
#[derive(Debug)]
pub struct ServiceResponse<T> {
    /// The HTTP version
    pub version: Version,
    /// The set of headers
    ///
    /// Should always at least have `Content-Type`. Servers will override `Content-Length` on serialization.
    pub headers: HeaderMap,
    /// The status code
    pub status: StatusCode,
    /// The serialized output object
    pub output: T,
}

impl<T> ServiceResponse<T> {
    /// Create new service request with the given input object
    /// 
    /// This automatically sets the `Content-Type` header as `application/protobuf`.
    pub fn new(output: T) -> ServiceResponse<T> { 
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/protobuf"));
        ServiceResponse {
            version: Version::default(),
            headers: headers,
            status: StatusCode::OK,
            output
        }
    }
    
    /// Copy this response with a different output value
    pub fn clone_with_output<U>(&self, output: U) -> ServiceResponse<U> {
        ServiceResponse { version: self.version, headers: self.headers.clone(), status: self.status, output }
    }
}

impl<T: Message + Default + 'static> From<T> for ServiceResponse<T> {
    fn from(v: T) -> ServiceResponse<T> { ServiceResponse::new(v) }
}

impl ServiceResponse<Vec<u8>> {
    /// Turn a hyper response to a boxed future of a byte-array service response
    pub fn from_hyper_raw(resp: Response) -> PTRes<Vec<u8>> {
        let version = resp.version();
        let headers = resp.headers().clone();
        let status = resp.status();
        Box::pin(body::to_bytes(resp).map_err(ProstTwirpError::HyperError).map(move |result| result.map(move |body| {
            ServiceResponse { version, headers, status, output: body.to_vec() }
        })))
    }

    /// Turn a byte-array service response into a hyper response
    pub fn to_hyper_raw(&self) -> Response {
        let mut resp = Response::new(self.output.clone().into());
        *resp.status_mut() = self.status;
        resp.headers_mut().clone_from(&self.headers);
        resp.headers_mut().insert(header::CONTENT_LENGTH, self.output.len().into());
        resp
    }

    /// Turn a byte-array service response into a `AfterBodyError`-wrapped version of the given error
    pub fn body_err(&self, err: ProstTwirpError) -> ProstTwirpError {
        ProstTwirpError::AfterBodyError {
            body: self.output.clone(), method: None, version: self.version,
            headers: self.headers.clone(), status: Some(self.status), err: Box::new(err)
        }
    }

    /// Serialize the byte-array service response into a protobuf service response
    pub fn to_proto<T: Message + Default + 'static>(&self) -> Result<ServiceResponse<T>, ProstTwirpError> {
        if self.status.is_success() {
            match T::decode(&*self.output) {
                Ok(v) => Ok(self.clone_with_output(v)),
                Err(err) => Err(self.body_err(ProstTwirpError::ProstDecodeError(err)))
            }
        } else {
            match TwirpError::from_json_bytes(self.status, &self.output) {
                Ok(err) => Err(self.body_err(ProstTwirpError::TwirpError(err))),
                Err(err) => Err(self.body_err(ProstTwirpError::JsonDecodeError(err)))
            }
        }
    }
}

impl<T: Message + Default + 'static> ServiceResponse<T> {
    /// Turn a protobuf service response into a byte-array service response
    pub fn to_proto_raw(&self) -> Result<ServiceResponse<Vec<u8>>, ProstTwirpError> {
        let mut body = Vec::new();
        if let Err(err) = self.output.encode(&mut body) {
            Err(ProstTwirpError::ProstEncodeError(err))
        } else {
            Ok(self.clone_with_output(body))
        }
    }

    /// Turn a hyper response into a protobuf service response
    pub fn from_hyper_proto(resp: Response) -> PTRes<T> {
        Box::pin(ServiceResponse::from_hyper_raw(resp).map(|r| r.and_then(|v| v.to_proto())))
    }

    /// Turn a protobuf service response into a hyper response
    pub fn to_hyper_proto(&self) -> Result<Response, ProstTwirpError> {
        self.to_proto_raw().map(|v| v.to_hyper_raw())
    }
}

/// A JSON-serializable Twirp error
#[derive(Debug)]
pub struct TwirpError {
    pub status: StatusCode,
    pub error_type: String,
    pub msg: String,
    pub meta: Option<serde_json::Value>,
}

impl TwirpError {
    /// Create a Twirp error with no meta
    pub fn new(status: StatusCode, error_type: &str, msg: &str) -> TwirpError {
        TwirpError::new_meta(status, error_type, msg, None)
    }

    /// Create a Twirp error with optional meta
    pub fn new_meta(status: StatusCode, error_type: &str, msg: &str, meta: Option<serde_json::Value>) -> TwirpError {
        TwirpError { status, error_type: error_type.to_string(), msg: msg.to_string(), meta }
    }

    /// Create a byte-array service response for this error and the given status code
    pub fn to_resp_raw(&self) -> ServiceResponse<Vec<u8>> {
        let output = self.to_json_bytes().unwrap_or_else(|_| "{}".as_bytes().to_vec());
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(header::CONTENT_LENGTH, output.len().into());
        ServiceResponse {
            version: Version::default(),
            headers: headers,
            status: self.status,
            output
        }
    }

    /// Create a hyper response for this error and the given status code
    pub fn to_hyper_resp(&self) -> Response {
        let body = self.to_json_bytes().unwrap_or_else(|_| "{}".as_bytes().to_vec());
        hyper::Response::builder().
            status(self.status).
            header(header::CONTENT_TYPE, HeaderValue::from_static("application/json")).
            header(header::CONTENT_LENGTH, HeaderValue::from(body.len())).
            body(body.into()).
            unwrap() // TODO: don't unwrap?
    }

    /// Create error from Serde JSON value
    pub fn from_json(status: StatusCode, json: serde_json::Value) -> TwirpError {
        let error_type = json["error_type"].as_str();
        TwirpError {
            status,
            error_type: error_type.unwrap_or("<no code>").to_string(),
            msg: json["msg"].as_str().unwrap_or("<no message>").to_string(),
            // Put the whole thing as meta if there was no type
            meta: if error_type.is_some() { json.get("meta").map(|v| v.clone()) } else { Some(json.clone()) },
        }
    }

    /// Create error from byte array
    pub fn from_json_bytes(status: StatusCode, json: &[u8]) -> serde_json::Result<TwirpError> {
        serde_json::from_slice(json).map(|v| TwirpError::from_json(status, v))
    }

    /// Create Serde JSON value from error
    pub fn to_json(&self) -> serde_json::Value {
        let mut props = serde_json::map::Map::new();
        props.insert("error_type".to_string(), serde_json::Value::String(self.error_type.clone()));
        props.insert("msg".to_string(), serde_json::Value::String(self.msg.clone()));
        if let Some(ref meta) = self.meta { props.insert("meta".to_string(), meta.clone()); }
        serde_json::Value::Object(props)
    }

    /// Create byte array from error
    pub fn to_json_bytes(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(&self.to_json())
    }
}

impl From<TwirpError> for ProstTwirpError {
    fn from(v: TwirpError) -> ProstTwirpError { ProstTwirpError::TwirpError(v) }
}

/// An error that can occur during a call to a Twirp service
#[derive(Debug)]
pub enum ProstTwirpError {
    /// A standard Twirp error with a type, message, and some metadata
    TwirpError(TwirpError),
    /// An error when trying to decode JSON into an error or object
    JsonDecodeError(serde_json::Error),
    /// An error when trying to encode a protobuf object
    ProstEncodeError(EncodeError),
    /// An error when trying to decode a protobuf object
    ProstDecodeError(DecodeError),
    /// A generic hyper error
    HyperError(hyper::Error),
    /// An error when trying to construct an URI and this shouldn't really happen.
    // TODO
    UriError(InvalidUri),
    /// A wrapper for any of the other `ProstTwirpError`s that also includes request/response info
    AfterBodyError {
        /// The request or response's raw body before the error happened
        body: Vec<u8>,
        /// The request method, only present for server errors
        method: Option<Method>,
        /// The request or response's HTTP version
        version: Version,
        /// The request or response's headers
        headers: HeaderMap,
        /// The response status, only present for client errors
        status: Option<StatusCode>,
        /// The underlying error
        err: Box<ProstTwirpError>,
    }
}

impl ProstTwirpError {
    /// This same error, or the underlying error if it is an `AfterBodyError`
    pub fn root_err(self) -> ProstTwirpError {
        match self {
            ProstTwirpError::AfterBodyError { err, .. } => err.root_err(),
            _ => self
        }
    }
}

/// A wrapper for a hyper client
#[derive(Debug)]
pub struct HyperClient {
    /// The hyper client
    pub client: Client<HttpConnector, Body>,
    /// The root URL without any path attached
    pub root_url: String,
}

impl HyperClient {
    /// Create a new client wrapper for the given client and root using protobuf
    pub fn new(client: Client<HttpConnector, Body>, root_url: &str) -> HyperClient {
        HyperClient {
            client,
            root_url: root_url.trim_end_matches('/').to_string(),
        }
    }

    /// Invoke the given request for the given path and return a boxed future result
    pub fn go<I, O>(&self, path: &str, req: ServiceRequest<I>) -> PTRes<O>
            where I: Message + Default + 'static, O: Message + Default + 'static {
        // Build the URI
        let uri = match format!("{}/{}", self.root_url, path.trim_start_matches('/')).parse() {
            Err(err) => return Box::pin(future::ready(Err(ProstTwirpError::UriError(err)))),
            Ok(v) => v,
        };
        // Build the request
        let mut hyper_req = match req.to_hyper_proto() {
            Err(err) => return Box::pin(future::ready(Err(err))),
            Ok(v) => v
        };
        *hyper_req.uri_mut() = uri;
        // Run the request and map the response
        Box::pin(self.client.request(hyper_req).
            map_err(ProstTwirpError::HyperError).
            and_then(ServiceResponse::from_hyper_proto))
    }
}

/// Service for taking a raw service request and returning a boxed future of a raw service response
pub trait HyperService {
    /// Accept a raw service request and return a boxed future of a raw service response
    fn handle(&self, req: ServiceRequest<Vec<u8>>) -> PTRes<Vec<u8>>;
}

/// A wrapper for a `HyperService` trait that keeps a `Arc` version of the service
pub struct HyperServer<T: 'static + HyperService> {
    /// The `Arc` version of the service
    /// 
    /// Needed because of [hyper Service lifetimes](https://github.com/tokio-rs/tokio-service/issues/9)
    pub service: Arc<T>
}

impl<T: 'static + HyperService> HyperServer<T> {
    /// Create a new service wrapper for the given impl
    pub fn new(service: T) -> HyperServer<T> { HyperServer { service: Arc::new(service) } }
}

impl<T: Send + Sync + 'static + HyperService> Service<Request> for HyperServer<T> {
    type Response = Response;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output=Result<Self::Response, Self::Error>>+Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(())) // TODO
    }

    fn call(&mut self, req: Request) -> Self::Future {
        if req.method() != &Method::POST {
            Box::pin(future::ready(Ok(TwirpError::new(StatusCode::METHOD_NOT_ALLOWED, "bad_method",
                "Method must be POST").to_hyper_resp())))
        } else if req.headers().get(header::CONTENT_TYPE) != Some(&HeaderValue::from_static("application/protobuf")) {
            Box::pin(future::ready(Ok(TwirpError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "bad_content_type", "Content type must be application/protobuf").to_hyper_resp())))
        } else {
            // Ug: https://github.com/tokio-rs/tokio-service/issues/9 // TODO
            let service = self.service.clone();
            Box::pin(ServiceRequest::from_hyper_raw(req).
                and_then(move |v| service.handle(v)).
                map(|r| r.map(|v| v.to_hyper_raw())).
                map(|r| r.or_else(|err| match err.root_err() {
                    ProstTwirpError::ProstDecodeError(_) =>
                        Ok(TwirpError::new(StatusCode::BAD_REQUEST, "protobuf_decode_err", "Invalid protobuf body").
                            to_hyper_resp()),
                    ProstTwirpError::TwirpError(err) =>
                        Ok(err.to_hyper_resp()),
                    // Just propagate hyper errors
                    ProstTwirpError::HyperError(err) =>
                        Err(err),
                    _ =>
                        Ok(TwirpError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_err", "Internal Error").
                            to_hyper_resp()),
                })))
        }
    }
}
