use std::sync::Arc;
use std::time::Duration;

use kuku::adapter::{
    discover_models, model_list_targets, trial_model, ModelTrial, Probe, ProbeBatch, ProbeProgress,
    ProbeResult, RequestCost, TRIAL_OUTPUT_TOKENS,
};
use kuku::config::{ApiKey, ProviderConfig, ProviderFormat, ProviderOptions, ProviderProxy};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

fn connection(base_url: String, proxy: ProviderProxy) -> ProviderConfig {
    ProviderConfig {
        format: ProviderFormat::OpenAiChat,
        base_url,
        api_key: ApiKey::Plaintext("fixture-only".into()),
        options: ProviderOptions {
            proxy,
            ..ProviderOptions::default()
        },
    }
}

async fn read_request(stream: &mut TcpStream) -> String {
    read_request_with_body(stream).await.0
}

async fn read_request_with_body(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    let mut first = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(0, reader.read_line(&mut line).await.unwrap());
        if line == "\r\n" {
            break;
        }
        if first.is_empty() {
            first.clone_from(&line);
        }
        request.push_str(&line);
    }
    let bytes = request
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; bytes];
    reader.read_exact(&mut body).await.unwrap();
    (first, body)
}

async fn response(stream: &mut TcpStream, body: &str) {
    stream.write_all(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ).as_bytes()).await.unwrap();
}

async fn socks_connection(stream: &mut TcpStream, remote_dns: bool) {
    let mut hello = [0; 2];
    stream.read_exact(&mut hello).await.unwrap();
    assert_eq!(5, hello[0]);
    let mut modes = vec![0; hello[1] as usize];
    stream.read_exact(&mut modes).await.unwrap();
    assert!(modes.contains(&0));
    stream.write_all(&[5, 0]).await.unwrap();
    let mut target = [0; 4];
    stream.read_exact(&mut target).await.unwrap();
    assert_eq!([5, 1, 0, if remote_dns { 3 } else { 1 }], target);
    if remote_dns {
        let length = stream.read_u8().await.unwrap();
        let mut name = vec![0; usize::from(length)];
        stream.read_exact(&mut name).await.unwrap();
        assert_eq!(b"provider-fixture.invalid", &name[..]);
    } else {
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!([127, 0, 0, 2], bytes);
    }
    assert_eq!(8123, stream.read_u16().await.unwrap());
    stream
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
        .await
        .unwrap();
}

#[tokio::test]
async fn discovery_and_model_requests_use_each_explicit_proxy() {
    for scheme in ["http", "socks5", "socks5h"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let dst = if scheme == "socks5h" {
            "provider-fixture.invalid"
        } else {
            "127.0.0.2"
        };
        let cfg = connection(
            format!("http://{dst}:8123/v1"),
            ProviderProxy::Explicit {
                url: format!("{scheme}://{address}"),
                auth: None,
            },
        );
        let mock = tokio::spawn(async move {
            for (method, path, body) in [
                ("GET", "/v1/models", r#"{"data":[{"id":"fixture"}]}"#),
                (
                    "POST",
                    "/v1/chat/completions",
                    r#"{"choices":[{"message":{"content":"377"}}],"usage":{"prompt_tokens":12,"completion_tokens":3,"cost":0.000023}}"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                if scheme != "http" {
                    socks_connection(&mut stream, scheme == "socks5h").await;
                }
                let (request, payload) = read_request_with_body(&mut stream).await;
                let path = if scheme == "http" {
                    format!("http://{dst}:8123{path}")
                } else {
                    path.into()
                };
                assert!(
                    request.starts_with(&format!("{method} {path} ")),
                    "{request:?}"
                );
                if method == "POST" {
                    let request: Value = serde_json::from_slice(&payload).unwrap();
                    assert_eq!(TRIAL_OUTPUT_TOKENS, request["max_tokens"]);
                }
                response(&mut stream, body).await;
            }
        });
        assert_eq!("fixture", discover_models(&cfg).await.unwrap()[0].id);
        let result = trial_model(
            &cfg,
            "fixture",
            ModelTrial::Text {
                effort: None,
                service_tier: None,
            },
        )
        .await
        .unwrap();
        assert!(result.text_returned);
        assert_eq!(
            RequestCost::Reported {
                nanodollars: 23_000
            },
            result.cost
        );
        tokio::time::timeout(Duration::from_secs(2), mock)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn batch_delivers_partial_catalogs_and_cancels_unsent_trials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cfg = connection(
        format!("http://{}", listener.local_addr().unwrap()),
        ProviderProxy::Direct,
    );
    let targets = model_list_targets(&cfg, true).unwrap();
    assert!(targets[0].url.ends_with("/v1/models"));
    assert!(targets[1].url.ends_with("/models"));
    let (requests, mut seen) = mpsc::channel(12);
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let requests = requests.clone();
            tokio::spawn(async move {
                let request = read_request(&mut stream).await;
                requests.send(request.clone()).await.unwrap();
                if request.starts_with("GET /v1/models ") {
                    response(&mut stream, r#"{"data":[{"id":"fixture"}]}"#).await;
                } else {
                    let _ = stream.read_u8().await;
                }
            });
        }
    });
    let mut discovery = ProbeBatch::start(targets.into_iter().map(Probe::from).collect());
    let first = tokio::time::timeout(Duration::from_secs(4), seen.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(4), seen.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(first.starts_with("GET") && second.starts_with("GET") && first != second);
    assert!(
        tokio::time::timeout(Duration::from_secs(4), discovery.next())
            .await
            .unwrap()
    );
    let (target, models) = discovery.useful_catalog().unwrap();
    assert!(target.connection.base_url.ends_with("/v1"));
    assert_eq!("fixture", models[0].id);
    assert!(discovery.pending());
    assert!(matches!(
        discovery.rows()[0].progress,
        ProbeProgress::Finished(_, Ok(ProbeResult::Catalog(_)))
    ));
    discovery.stop();

    let cfg = Arc::new(cfg);
    let mut trials = ProbeBatch::start(
        (0..3)
            .map(|_| Probe::Trial {
                connection: Arc::clone(&cfg),
                model: "fixture".into(),
                check: ModelTrial::Text {
                    effort: None,
                    service_tier: None,
                },
            })
            .collect(),
    );
    for _ in 0..2 {
        let request = tokio::time::timeout(Duration::from_secs(4), seen.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(request.starts_with("POST "));
    }
    assert!(matches!(trials.rows()[2].progress, ProbeProgress::Queued));
    trials.stop();
    assert!(matches!(
        trials.rows()[2].progress,
        ProbeProgress::Canceled { sent: false }
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), seen.recv())
            .await
            .is_err()
    );
    server.abort();
}
