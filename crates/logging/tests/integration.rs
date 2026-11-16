#![cfg(test)]

#[test]
fn it_initializes_and_logs() {
    log::info!("integration test log info");
    log::error!("integration test log error");
}
