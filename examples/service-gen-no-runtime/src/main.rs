use std::convert::Infallible;
use std::env;
use std::time::Duration;
use futures::future;
use hyper::Client;
use hyper::server::Server;
use hyper::service::make_service_fn;
use tokio::time;
use tokio::sync::oneshot;

mod service {
    pub use prost_twirp::ProstTwirpError;
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
        let service_client = <dyn service::Haberdasher>::new_client(hyper_client, "http://localhost:8080");
        // Run the 5 like the other client
        let work = future::join_all((0..5).map(|_| async {
            let res = service_client.make_hat(service::Size { inches: 12 }.into()).await?;
            let hat: service::Hat = res.output;
            Ok::<(), service::ProstTwirpError>(println!("Made {:?}", hat))
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
        Box::pin(future::ok(
            service::Hat { size: i.input.inches, color: "blue".to_string(), name: "fedora".to_string() }.into()
        ))
    }
}
