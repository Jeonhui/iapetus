use std::io::Result;

/// Generates Rust types from the `.proto` files, which are the single source of
/// truth for the wire format (PRD §19.1). Nothing generated here is committed.
fn main() -> Result<()> {
    let protos = [
        "proto/iapetus/v1/common.proto",
        "proto/iapetus/v1/desktop.proto",
        "proto/iapetus/v1/session.proto",
        "proto/iapetus/v1/action.proto",
        "proto/iapetus/v1/event.proto",
        "proto/iapetus/v1/daemon.proto",
    ];

    for p in &protos {
        println!("cargo:rerun-if-changed={p}");
    }

    let cfg = tonic_build::configure()
        // Only daemon.proto declares a service, and only the grpc feature needs it.
        .build_server(cfg!(feature = "grpc"))
        .build_client(cfg!(feature = "grpc"));

    // prost already derives Hash, Eq, PartialOrd, and Ord on enums, so adding
    // them here conflicts. Left as a note so nobody re-adds it.

    cfg.compile_protos(&protos, &["proto"])
}
