use futures::{Future, Poll};
use rustls::ClientSession;
use std::sync::Arc;
use tokio::executor::DefaultExecutor;
use tokio::net::tcp::TcpStream;
use tokio_rustls::{rustls::ClientConfig, Connect, TlsConnector, TlsStream};
use tower_grpc::Request;
use tower_h2::client;
use tower_service::Service;
use tower_util::MakeService;
use webpki::DNSNameRef;

pub mod spike_proto {
    include!(concat!(env!("OUT_DIR"), "/spike.rs"));
}

pub fn main() {
    let _ = ::env_logger::init();

    let uri: http::Uri = format!("https://0.0.0.0:10011").parse().unwrap();

    let h2_settings = h2::client::Builder::new();
    let mut make_client = client::Connect::new(Dst, h2_settings, DefaultExecutor::current());

    let do_send = make_client
        .make_service(())
        .map(move |conn| {
            use spike_proto::client::Spike;

            let conn = tower_request_modifier::Builder::new()
                .set_origin(uri)
                .build(conn)
                .unwrap();

            Spike::new(conn)
        })
        .and_then(|mut client| {
            use spike_proto::SendRequest;
            client
                .send(Request::new(SendRequest {
                    room: "ninety-two".into(),
                    msg: "not cheap, but costly".into(),
                }))
                .map_err(|e| panic!("gRPC request failed; err={:?}", e))
        })
        .and_then(|response| {
            println!("RESPONSE = {:?}", response);
            Ok(())
        })
        .map_err(|e| {
            println!("ERR = {:?}", e);
        });

    tokio::run(do_send);
}

struct Dst;

impl Service<()> for Dst {
    type Response = TlsStream<TcpStream, ClientSession>;
    type Error = ::std::io::Error;
    type Future = Connect<TcpStream>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        Ok(().into())
    }

    fn call(&mut self, _: ()) -> Self::Future {
        let mut config = ClientConfig::new();
        config
            .root_store
            .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
        config.alpn_protocols.push(b"h2".to_vec());
        let tls_connector = TlsConnector::from(Arc::new(config));

        let domain = DNSNameRef::try_from_ascii_str("0.0.0.0:10011").unwrap();
        let addr = "0.0.0.0:10011".parse().unwrap();
        TcpStream::connect(&addr).and_then(move |sock| {
            sock.set_nodelay(true).unwrap();
            tls_connector.connect(domain, sock)
        })
    }
}
