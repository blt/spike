extern crate tower_grpc_build;

fn main() {
    tower_grpc_build::Config::new()
        .enable_server(true)
        .enable_client(true)
        .build(&["resources/proto/spike.proto"], &["resources/proto/"])
        .unwrap_or_else(|e| panic!("protobuf compilation failed: {}", e));
}
