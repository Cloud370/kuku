use super::{options, script_facts, start, until};
use kuku::{AgentStatus, OutputRead, Session};
use serde_json::json;

#[tokio::test]
async fn oversized_script_return_is_readable_after_reopen_and_respects_output_quota() {
    let code = "return {large:'x'.repeat(20000), kind:'report'};";
    for retention_available in [true, false] {
        let work = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut config = options(&work, &data, code);
        if !retention_available {
            config.limits.max_output_bytes = 4096;
        }
        let session = start(config.clone()).await;
        until(|| (session.agent("main").ok()?.status == AgentStatus::Idle).then_some(())).await;
        let result = script_facts(&session).unwrap();
        assert!(result.value.is_null());
        assert_eq!(result.output_ref.is_some(), retention_available);
        config.session_id = Some(session.id().into());
        session.close().await.unwrap();

        if let Some(output) = result.output_ref {
            let reopened = Session::open(config).await.unwrap();
            let page = reopened
                .read_agent_output(
                    "main",
                    &output.id,
                    &OutputRead {
                        offset: 0,
                        limit: output.bytes as usize,
                        search: None,
                    },
                )
                .unwrap();
            let text: String = page
                .segments
                .iter()
                .map(|part| part.text.as_str())
                .collect();
            assert_eq!(
                json!({"large":"x".repeat(20000),"kind":"report"}),
                serde_json::from_str::<serde_json::Value>(&text).unwrap()
            );
            reopened.close().await.unwrap();
        }
    }
}
