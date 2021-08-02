use std::convert::Infallible;
use std::env;
use std::time::Duration;

use futures::future;
use hyper::{Client, StatusCode};
use hyper::server::Server;
use hyper::service::make_service_fn;
use prost_twirp::TwirpError;
use tokio::sync::oneshot;
use tokio::time;

mod service {
    include!(concat!(env!("OUT_DIR"), "/twitch.twirp.example.rs"));
}

#[tokio::main]
async fn main() {
    let run_server = env::args().any(|s| s == "--server");
    let run_client = !run_server || env::args().any(|s| s == "--client");
    let (shutdown_send, shutdown_recv) = oneshot::channel::<()>();

    if run_server {
        let thread_res = tokio::spawn(async {
            println!("Starting server");
            let addr = "0.0.0.0:8080".parse().unwrap();
            let make_service = make_service_fn(|_conn| async {
                let service = <dyn service::Haberdasher>::new_server(HaberdasherService);
                Ok::<_, Infallible>(service)
            });
            let server = Server::bind(&addr)
                .serve(make_service)
                .with_graceful_shutdown(async { drop(shutdown_recv.await); });
            server.await.unwrap();
            println!("Server stopped");
        });
        // Wait a sec or forever depending on whether there's client code to run
        if run_client {
            time::sleep(Duration::from_millis(1000)).await;
        } else {
            if let Err(err) = thread_res.await { println!("Server panicked: {:?}", err); }
        }
    }

    if run_client {
        let hyper_client = Client::new();
        let service_client = <dyn service::Haberdasher>::new_client(hyper_client.clone(), "http://localhost:8080");
        // Try one too small, then too large, then just right
        let work = future::join_all([0, 11, 5].map(|inches| {
            let service_client = &service_client;
            async move {
                let res = service_client.make_hat(service::Size { inches }.into()).await;
                let res = res.map(|v| v.output).map_err(|e| e.root_err());
                Ok::<(), ()>(println!("For size {}: {:?}", inches, res))
            }
        }));
        for result in work.await {
            result.unwrap();
        }
        drop(shutdown_send);
    }
}

pub struct HaberdasherService;
impl service::Haberdasher for HaberdasherService {
    fn make_hat(&self, i: service::PTReq<service::Size>) -> service::PTRes<service::Hat> {
        Box::pin(async move {
            if i.input.inches < 1 {
                Err(TwirpError::new_meta(StatusCode::BAD_REQUEST, "too_small", "Size too small",
                    serde_json::to_value(MinMaxSize { min: 1, max: 10 }).ok()).into())
            } else if i.input.inches > 10 {
                Err(TwirpError::new_meta(StatusCode::BAD_REQUEST, "too_large", "Size too large",
                    serde_json::to_value(MinMaxSize { min: 1, max: 10 }).ok()).into())
            } else {
                Ok(service::Hat { size: i.input.inches, color: "blue".to_string(), name: "fedora".to_string() }.into())
            }
        })
    }
}

#[derive(serde_derive::Serialize, serde_derive::Deserialize, Debug)]
struct MinMaxSize { min: i32, max: i32 }
