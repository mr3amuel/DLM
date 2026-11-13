// Separate executable target: it imports protocol/host code, never the download engine.
#[path = "../browser_host.rs"]
mod browser_host;
#[path = "../browser_protocol.rs"]
mod browser_protocol;
#[path = "../chrome_setup.rs"]
mod chrome_setup;
fn main() {
    browser_host::main();
}
