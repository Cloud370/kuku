use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};

use tokio::runtime::{Handle, Id};
use wreq::Client;
use wreq_util::Emulation;

#[derive(Default)]
struct RuntimeClients {
    api: OnceLock<Client>,
    fetch: OnceLock<Client>,
}

fn runtime_clients() -> Arc<RuntimeClients> {
    static RUNTIMES: LazyLock<Mutex<HashMap<Id, Weak<RuntimeClients>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let runtime = Handle::current();
    let mut runtimes = RUNTIMES.lock().unwrap();
    if let Some(clients) = runtimes.get(&runtime.id()).and_then(Weak::upgrade) {
        return clients;
    }
    runtimes.retain(|_, entry| entry.strong_count() > 0);
    let clients = Arc::new(RuntimeClients::default());
    runtimes.insert(runtime.id(), Arc::downgrade(&clients));
    let owner = Arc::clone(&clients);
    // NOTE: The runtime releases its connection pools when it drops its tasks.
    runtime.spawn(async move {
        let _clients = owner;
        std::future::pending::<()>().await;
    });
    clients
}

pub(crate) fn api_client() -> Client {
    runtime_clients()
        .api
        .get_or_init(|| {
            Client::builder()
                .user_agent(concat!("kuku/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("default HTTP client builds")
        })
        .clone()
}

pub(crate) fn fetch_client() -> Client {
    runtime_clients()
        .fetch
        .get_or_init(|| {
            Client::builder()
                .emulation(Emulation::Chrome136)
                .build()
                .expect("emulation HTTP client builds")
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use httpmock::prelude::{MockServer, POST};
    use tokio::runtime::Builder;

    #[test]
    fn paused_runtime_does_not_block_requests_from_another_runtime() {
        let server = MockServer::start();
        let reply = server.mock(|when, then| {
            when.method(POST).path("/runtime-pool");
            then.status(200).body("ok");
        });
        let first = Builder::new_current_thread().enable_all().build().unwrap();
        let second = Builder::new_current_thread().enable_all().build().unwrap();
        let first_pools = first.block_on(async {
            let pools = super::runtime_clients();
            assert!(Arc::ptr_eq(&pools, &super::runtime_clients()));
            Arc::downgrade(&pools)
        });
        for runtime in [&first, &second, &first] {
            runtime.block_on(async {
                for client in [super::api_client(), super::fetch_client()] {
                    let response = tokio::time::timeout(Duration::from_secs(5), async {
                        client
                            .post(server.url("/runtime-pool"))
                            .send()
                            .await
                            .unwrap()
                            .text()
                            .await
                            .unwrap()
                    })
                    .await
                    .expect("request must progress on its own runtime");
                    assert_eq!("ok", response);
                }
            });
        }
        drop(first);
        assert!(first_pools.upgrade().is_none());
        reply.assert_hits(6);
    }
}
