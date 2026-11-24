use std::sync::Arc;
use std::time::Duration;

use kuku::ProviderFailureKind;
use kuku::adapter::{
    ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, ReasoningCheck,
    ReasoningCheckResult, RequestCost, SetupTextCheck, TRIAL_OUTPUT_TOKENS, discover_models,
    model_list_targets, trial_model,
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

#[tokio::test]
async fn tool_trial_requires_a_structured_call_and_a_reply_to_the_tool_result() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = connection(
        format!("http://{}/v1", listener.local_addr().unwrap()),
        ProviderProxy::Direct,
    );
    config.format = ProviderFormat::OpenAiResponses;
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let (headers, body) = read_request_with_body(&mut first).await;
        assert!(headers.starts_with("POST /v1/responses "));
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!("kuku_setup_check", body["tools"][0]["name"]);
        response(&mut first, r#"{"output":[{"type":"function_call","call_id":"call_setup","name":"kuku_setup_check","arguments":"{}"}]}"#).await;
        let (mut next, _) = listener.accept().await.unwrap();
        let (_, body) = read_request_with_body(&mut next).await;
        let body: Value = serde_json::from_slice(&body).unwrap();
        let last = body["input"].as_array().unwrap().last().unwrap();
        assert_eq!("function_call_output", last["type"]);
        assert_eq!("call_setup", last["call_id"]);
        let answer = &last["output"];
        response(&mut next, &serde_json::json!({"output":[{"type":"message","content":[{"type":"output_text","text":answer}]}]}).to_string()).await;
    });
    let observed = trial_model(&config, "fixture", ModelTrial::Tools { service_tier: None })
        .await
        .unwrap();
    assert_eq!(Some(true), observed.tool_round_trip);
    server.await.unwrap();
}

#[tokio::test]
async fn model_trial_times_out_after_thirty_seconds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let mut batch = ProbeBatch::start(vec![Probe::Trial {
        connection: Arc::new(connection(base, ProviderProxy::Direct)),
        model: "fixture".into(),
        check: ModelTrial::Text {
            effort: None,
            service_tier: None,
        },
    }]);
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(5), read_request(&mut stream))
        .await
        .unwrap();
    assert!(request.starts_with("POST /v1/chat/completions "));

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(29)).await;
    assert_eq!(batch.text_check("fixture"), SetupTextCheck::Testing);
    assert!(!batch.poll());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), batch.next())
            .await
            .unwrap()
    );
    assert!(matches!(
        batch.text_check("fixture"),
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::Timeout),
            ..
        }
    ));
}

#[tokio::test]
async fn api_discovery_tries_another_route_after_an_explicit_404() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        assert!(
            read_request(&mut first)
                .await
                .starts_with("POST /v1/responses ")
        );
        first
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let (mut second, _) = listener.accept().await.unwrap();
        let (request, body) = read_request_with_body(&mut second).await;
        assert!(request.starts_with("POST /v1/chat/completions "));
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["model"], "fixture-model");
        assert_eq!(body["max_tokens"], TRIAL_OUTPUT_TOKENS);
        response(
            &mut second,
            r#"{"choices":[{"message":{"content":"377"},"finish_reason":"stop"}]}"#,
        )
        .await;
    });
    let candidates = vec![ProviderFormat::OpenAiResponses, ProviderFormat::OpenAiChat];
    let mut batch = ProbeBatch::start(vec![Probe::ApiType {
        connection: Arc::new(connection(base, ProviderProxy::Direct)),
        model: "fixture-model".into(),
        candidates: candidates.clone(),
    }]);
    tokio::time::timeout(Duration::from_secs(5), batch.next())
        .await
        .unwrap();
    match &batch.rows()[0].progress {
        ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) => {
            assert_eq!(evidence.attempted, candidates);
            assert_eq!(evidence.format, ProviderFormat::OpenAiChat);
            assert!(evidence.trial.text_returned);
        }
        ProbeProgress::Finished(_, Err(error)) => panic!("API discovery failed: {error}"),
        _ => panic!("Missing API discovery result"),
    }
    server.await.unwrap();
}

#[tokio::test]
async fn api_discovery_starts_all_protocols_and_respects_preference() {
    let formats = [
        ProviderFormat::OpenAiResponses,
        ProviderFormat::OpenAiChat,
        ProviderFormat::Anthropic,
    ];
    for (responses_status, chat_status, selected) in [
        (200, 200, Some(ProviderFormat::OpenAiResponses)),
        (404, 200, Some(ProviderFormat::OpenAiChat)),
        (404, 404, Some(ProviderFormat::Anthropic)),
        (401, 200, None),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let mut batch = ProbeBatch::start_api_check(
            &connection(base, ProviderProxy::Direct),
            "fixture-model",
            &[formats[2], formats[1], formats[0]],
        );

        let mut streams = tokio::time::timeout(Duration::from_secs(5), async {
            let mut streams = Vec::new();
            for _ in formats {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (request, body) = read_request_with_body(&mut stream).await;
                let path = request.split_ascii_whitespace().nth(1).unwrap().to_owned();
                let body: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!("fixture-model", body["model"]);
                streams.push((path, stream));
            }
            streams.sort_by(|left, right| left.0.cmp(&right.0));
            streams
        })
        .await
        .expect("all three requests must arrive before any reply");
        assert_eq!(
            ["/v1/chat/completions", "/v1/messages", "/v1/responses"],
            std::array::from_fn(|index| streams[index].0.as_str())
        );
        let (_, mut responses) = streams.pop().unwrap();
        let (_, mut messages) = streams.pop().unwrap();
        let (_, mut chat) = streams.pop().unwrap();

        response(
            &mut messages,
            r#"{"content":[{"type":"text","text":"377"}],"stop_reason":"end_turn"}"#,
        )
        .await;
        if chat_status == 200 {
            response(
                &mut chat,
                r#"{"choices":[{"message":{"content":"377"},"finish_reason":"stop"}]}"#,
            )
            .await;
        } else {
            error_status(&mut chat, chat_status).await;
        }
        for _ in 0..2 {
            assert!(
                tokio::time::timeout(Duration::from_secs(5), batch.next())
                    .await
                    .unwrap()
            );
            assert!(!batch.finish_api_check());
        }

        if responses_status == 200 {
            response(
                &mut responses,
                r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"377"}]}]}"#,
            )
            .await;
        } else {
            error_status(&mut responses, responses_status).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(5), batch.next())
                .await
                .unwrap()
        );
        assert!(batch.finish_api_check());
        assert_eq!(1, batch.selected_rows().count());
        let selected_format = match &batch.rows()[0].progress {
            ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) => {
                assert!(evidence.trial.text_returned);
                Some(evidence.format)
            }
            ProbeProgress::Finished(_, Err(_)) => None,
            _ => panic!("missing final API discovery result"),
        };
        assert_eq!(selected, selected_format);
    }
}

async fn error_status(stream: &mut TcpStream, status: u16) {
    stream
        .write_all(
            format!("HTTP/1.1 {status} Rejected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn thinking_test_waits_for_retry_after_and_persists_request_acceptance() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let route = Arc::new(connection(
        format!("http://{}/v1", listener.local_addr().unwrap()),
        ProviderProxy::Direct,
    ));
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (_, body) = read_request_with_body(&mut stream).await;
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["reasoning_effort"],
                "none"
            );
            if attempt == 0 {
                stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            } else {
                response(&mut stream, r#"{"choices":[{"message":{"content":"377","reasoning_content":"fixture"},"finish_reason":"stop"}]}"#).await;
            }
        }
    });
    let mut batch = ProbeBatch::start(vec![Probe::Thinking {
        connection: Arc::clone(&route),
        model: "fixture".into(),
        check: ReasoningCheck::Level("none".into()),
        service_tier: None,
    }]);
    assert!(batch.next().await);
    let ProbeProgress::RetryWaiting { until, sent: true } = batch.rows()[0].progress else {
        panic!("expected a scheduled retry")
    };
    assert!(until.saturating_duration_since(std::time::Instant::now()) <= Duration::from_secs(1));
    assert!(batch.pending());
    tokio::time::timeout(Duration::from_secs(4), async {
        while batch.next().await {}
    })
    .await
    .unwrap();
    let ProbeProgress::Finished(
        _,
        Ok(ProbeResult::Thinking(ReasoningCheckResult::Accepted(reply))),
    ) = &batch.rows()[0].progress
    else {
        panic!("expected a completed thinking probe")
    };
    assert!(reply.observed_reasoning);
    let test = batch.thinking_test(&route, "fixture").unwrap();
    assert_eq!(test.accepted, ["none"]);
    let mut other_endpoint = route.as_ref().clone();
    other_endpoint.base_url.push_str("/other");
    assert!(batch.thinking_test(&other_endpoint, "fixture").is_none());
    server.await.unwrap();
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
