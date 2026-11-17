use std::sync::OnceLock;

use wreq::Client;
use wreq_util::Emulation;

pub(crate) fn api_client() -> Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent(concat!("kuku/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("default HTTP client builds")
        })
        .clone()
}

pub(crate) fn fetch_client() -> Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .emulation(Emulation::Chrome136)
                .build()
                .expect("emulation HTTP client builds")
        })
        .clone()
}
