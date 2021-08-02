use std::convert::Infallible;
use std::env;
use std::time::Duration;
use futures::future;
use hyper::{Client, Method, StatusCode};
use hyper::server::Server;
use hyper::service::make_service_fn;
use prost_twirp::{PTRes, HyperClient, HyperServer, HyperService, ServiceRequest, ServiceResponse, TwirpError, ProstTwirpError};
use tokio::time;
use tokio::sync::oneshot;

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
                Ok::<_, Infallible>(HyperServer::new(MyServer))
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
        let prost_client = HyperClient::new(hyper_client, "http://localhost:8080");
        // Run the 5 like the other client
        let work = future::join_all((0..5).map(|_| async {
            let res = prost_client.
                go("/twirp/twitch.twirp.example.Haberdasher/MakeHat",
                    ServiceRequest::new(service::Size { inches: 12 })).
                await?;
            let hat: service::Hat = res.output;
            Ok::<(), ProstTwirpError>(println!("Made {:?}", hat))
        }));
        for result in work.await {
            result.unwrap();
        }
        drop(shutdown_send);
    }
}

struct MyServer;
impl HyperService for MyServer {
    fn handle(&self, req: ServiceRequest<Vec<u8>>) -> PTRes<Vec<u8>> {
        match (req.method.clone(), req.uri.path()) {
            (Method::POST, "/twirp/twitch.twirp.example.Haberdasher/MakeHat") =>
                Box::pin(std::future::ready(req.to_proto().and_then(|req| {
                    let size: service::Size = req.input;
                    ServiceResponse::new(
                        service::Hat { size: size.inches, color: "blue".to_string(), name: "fedora".to_string() }
                    ).to_proto_raw()
                }))),
            _ => Box::pin(future::ok(TwirpError::new(StatusCode::NOT_FOUND, "not_found", "Not found").to_resp_raw()))
        }
    }
}
