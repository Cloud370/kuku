use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};

use lru::LruCache;
use sha2::{Digest, Sha256};
use tokio::runtime::{Handle, Id};
use wreq::Client;
use wreq_util::Emulation;

use crate::config::ProviderProxy;
use crate::{Error, Result};

type ProxyPool = LruCache<(ProviderProxy, Option<[u8; 32]>), Client>;
type CatalogSlots = LruCache<
    (String, ProviderProxy, [u8; 32]),
    Arc<tokio::sync::Mutex<super::discovery::CachedCatalog>>,
>;

#[derive(Default)]
struct RuntimeClients {
    api: OnceLock<Client>,
    direct: OnceLock<Client>,
    proxies: OnceLock<Mutex<ProxyPool>>,
    catalogs: OnceLock<Mutex<CatalogSlots>>,
    fetch: OnceLock<Client>,
}

pub(super) fn catalog_slot(
    url: String,
    proxy: ProviderProxy,
    credential: &str,
) -> Arc<tokio::sync::Mutex<super::discovery::CachedCatalog>> {
    let clients = runtime_clients();
    let mut entries = clients
        .catalogs
        .get_or_init(|| Mutex::new(LruCache::new(NonZeroUsize::new(8).unwrap())))
        .lock()
        .unwrap();
    entries
        .get_or_insert(
            (url, proxy, Sha256::digest(credential).into()),
            Arc::default,
        )
        .clone()
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

fn api_builder() -> wreq::ClientBuilder {
    Client::builder().user_agent(concat!("kuku/", env!("CARGO_PKG_VERSION")))
}

pub(crate) fn api_client(settings: &ProviderProxy) -> Result<Client> {
    let clients = runtime_clients();
    let (url, auth) = match settings {
        ProviderProxy::Environment => {
            return Ok(clients
                .api
                .get_or_init(|| api_builder().build().expect("default HTTP client builds"))
                .clone());
        }
        ProviderProxy::Direct => {
            return Ok(clients
                .direct
                .get_or_init(|| {
                    api_builder()
                        .no_proxy()
                        .build()
                        .expect("direct HTTP client builds")
                })
                .clone());
        }
        ProviderProxy::Explicit { url, auth } => (url, auth),
    };
    settings.validate()?;
    let password = auth
        .as_ref()
        .map(|auth| auth.password_reference()?.resolve())
        .transpose()?;
    let fingerprint = password
        .as_ref()
        .map(|password| Sha256::digest(password).into());
    let mut pools = clients
        .proxies
        .get_or_init(|| Mutex::new(LruCache::new(NonZeroUsize::new(16).unwrap())))
        .lock()
        .unwrap();
    let cache_key = (settings.clone(), fingerprint);
    if let Some(client) = pools.get(&cache_key) {
        return Ok(client.clone());
    }
    let mut proxy = wreq::Proxy::all(url.as_str())
        .map_err(|_| Error::ConfigLoad("Invalid provider proxy address.".into()))?;
    if let Some((auth, password)) = auth.as_ref().zip(password) {
        proxy = proxy.basic_auth(&auth.username, &password);
    }
    let client = api_builder()
        .proxy(proxy)
        .build()
        .map_err(|_| Error::ConfigLoad("Could not configure the provider proxy.".into()))?;
    pools.put(cache_key, client.clone());
    Ok(client)
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
                for client in [
                    super::api_client(&crate::config::ProviderProxy::Environment).unwrap(),
                    super::fetch_client(),
                ] {
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
