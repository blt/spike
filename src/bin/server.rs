#[macro_use]
extern crate log;

use futures::sync::mpsc;
use futures::{future, Future, Sink, Stream};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::executor::DefaultExecutor;
use tokio::net::TcpListener;
use tokio_rustls::{
    rustls::{AllowAnyAuthenticatedClient, RootCertStore, ServerConfig},
    TlsAcceptor,
};
use tower_grpc::{Code, Request, Response, Status};

pub mod spike_proto {
    include!(concat!(env!("OUT_DIR"), "/spike.rs"));
}

#[derive(Clone, Debug)]
struct Spike {
    state: Arc<State>,
}

#[derive(Debug)]
struct State {
    rooms: Mutex<HashMap<String, Vec<String>>>,
}

impl spike_proto::server::Spike for Spike {
    type SendFuture = future::FutureResult<Response<spike_proto::SendResponse>, tower_grpc::Status>;
    fn send(&mut self, request: Request<spike_proto::SendRequest>) -> Self::SendFuture {
        let send_request: spike_proto::SendRequest = request.into_inner();

        let mut rooms = self.state.rooms.lock().unwrap();
        let room = rooms
            .entry(send_request.room)
            .or_insert_with(|| Vec::with_capacity(16));
        room.push(send_request.msg);

        let response = Response::new(spike_proto::SendResponse {});

        future::ok(response)
    }

    type WatchStream =
        Box<Stream<Item = spike_proto::WatchResponse, Error = tower_grpc::Status> + Send>;
    type WatchFuture = future::FutureResult<Response<Self::WatchStream>, tower_grpc::Status>;

    fn watch(&mut self, request: Request<spike_proto::WatchRequest>) -> Self::WatchFuture {
        use std::thread;

        let (tx, rx) = mpsc::channel(4);

        let watch_request: spike_proto::WatchRequest = request.into_inner();

        let rooms = self.state.rooms.lock().unwrap();
        let room: Option<&Vec<String>> = rooms.get(&watch_request.room);
        if room.is_none() {
            return future::err(Status::new(Code::NotFound, "room unknown"));
        }
        // We clone only the messages for the room. It'd be better if we didn't
        // have to clone at all.
        let mut room: Vec<String> = room.unwrap().clone();
        drop(rooms);

        // It's a little unfortunate that we spawn a thread for this. I wonder,
        // can we avoid this thread?
        thread::spawn(move || {
            let mut tx = tx.wait();

            // TODO don't unwrap, signal error back up
            for msg in room.drain(..) {
                tx.send(spike_proto::WatchResponse { msg }).unwrap();
            }
        });

        let rx = rx.map_err(|_| unimplemented!());
        future::ok(Response::new(Box::new(rx)))
    }
}

pub fn main() {
    ::env_logger::init();

    let threads = num_cpus::get();
    let spike = Spike {
        state: Arc::new(State {
            rooms: Mutex::new(HashMap::new()),
        }),
    };

    let service = spike_proto::server::SpikeServer::new(spike);

    let h2_settings = Default::default();
    let h2 = Arc::new(Mutex::new(tower_h2::Server::new(
        service,
        h2_settings,
        DefaultExecutor::current(),
    )));

    let mut root_cert_store = RootCertStore::empty();
    root_cert_store.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    let config = ServerConfig::new(AllowAnyAuthenticatedClient::new(root_cert_store));
    let tls_acceptor = TlsAcceptor::from(Arc::new(config));

    let addr = "0.0.0.0:10011".parse().unwrap();
    let bind = TcpListener::bind(&addr).expect("bind");

    let serve = bind
        .incoming()
        .for_each(move |tls_sock| {
            let addr = tls_sock.peer_addr().ok();
            if let Err(e) = tls_sock.set_nodelay(true) {
                return Err(e);
            }
            info!("New connection from addr={:?}", addr);
            let h2_inner = h2.clone();
            let done = tls_acceptor
                .accept(tls_sock)
                .and_then(move |sock| {
                    let serve = h2_inner.lock().unwrap().serve(sock);
                    tokio::spawn(serve.map_err(|e| error!("h2 error: {:?}", e)));

                    Ok(())
                })
                .map_err(move |err| error!("TLS error: {:?} - {:?}", err, addr));
            tokio::spawn(done);

            Ok(())
        })
        .map_err(|e| error!("accept error: {}", e));

    let mut rt = tokio::runtime::Builder::new()
        .core_threads(threads)
        .build()
        .unwrap();

    rt.spawn(serve);
    info!(
        "Started server with {} threads, listening on {}",
        threads, addr
    );
    rt.shutdown_on_idle().wait().unwrap();
}
