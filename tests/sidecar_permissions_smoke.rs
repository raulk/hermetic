use undercover::sidecar::{PermissionSmoke, Sidecar};

#[tokio::test]
async fn sidecar_denies_network_write_and_broad_env() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binding local probe socket");
    let node_net_port = listener.local_addr().expect("local listener addr").port();

    let mut sidecar = Sidecar::spawn(std::path::Path::new("."))
        .await
        .expect("spawning sidecar");
    let smoke: PermissionSmoke = sidecar
        .call(
            "sidecar-permissions-smoke",
            serde_json::json!({ "node_net_port": node_net_port }),
        )
        .await
        .expect("running permission smoke");

    assert!(smoke.fetch_denied);
    assert!(smoke.connect_denied);
    assert!(smoke.node_net_denied);
    assert!(smoke.write_denied);
    assert!(smoke.env_denied);
    assert!(smoke.read_allowed);

    sidecar.shutdown().await.expect("sidecar shutdown");
}
