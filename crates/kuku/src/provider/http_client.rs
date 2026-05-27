use wreq::Client;
use wreq_util::Emulation;

pub(crate) fn api_client() -> Client {
    Client::builder()
        .user_agent(concat!("kuku/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("default HTTP client builds")
}

pub(crate) fn fetch_client() -> Client {
    Client::builder()
        .emulation(Emulation::Chrome136)
        .build()
        .expect("emulation HTTP client builds")
}
