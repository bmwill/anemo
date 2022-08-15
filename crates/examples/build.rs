fn main() {
    let greeter_service = anemo_build::manual::Service::builder()
        .name("Greeter")
        .package("json.helloworld")
        .method(
            anemo_build::manual::Method::builder()
                .name("say_hello")
                .route_name("SayHello")
                .request_type("crate::HelloRequest")
                .response_type("crate::HelloResponse")
                .codec_path("anemo::rpc::codec::BincodeCodec")
                // .codec_path("anemo::rpc::codec::JsonCodec")
                .build(),
        )
        .method(
            anemo_build::manual::Method::builder()
                .name("say_hello_2")
                .route_name("SayHello2")
                .request_type("crate::HelloRequest")
                .response_type("()")
                // .codec_path("anemo::rpc::codec::BincodeCodec")
                .codec_path("anemo::rpc::codec::JsonCodec")
                .build(),
        )
        .build();

    anemo_build::manual::Builder::new().compile(&[greeter_service]);
}
