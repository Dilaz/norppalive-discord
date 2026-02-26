fn main() {
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(false)
        .compile_protos(
            &[
                "proto/norppalive/v1/bot.proto",
                "proto/norppalive/v1/settings.proto",
            ],
            &["proto"],
        )
        .unwrap();
}
