use std::process::Command;

#[test]
fn spike_source_contains_connector_counter_assertion() {
    let transport = std::fs::read_to_string("src/transport.rs").unwrap();
    let spike = std::fs::read_to_string("examples/spike.rs").unwrap();
    let commands = std::fs::read_to_string("src/commands.rs").unwrap();

    assert!(transport.contains("TOR_CONNECT_CALLS.fetch_add"));
    assert!(spike.contains("provider call completed without Tor connector use"));
    assert!(commands.contains("without Tor connector use"));
}

#[test]
fn spike_example_builds() {
    let status = Command::new("cargo")
        .args(["build", "--example", "spike"])
        .status()
        .expect("running cargo build --example spike");
    assert!(status.success());
}
