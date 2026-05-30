use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};

use super::hook::HookInstance;
use super::output::{handle_overflow, merge_outputs, parse_stdout, HookOutput};

const SIGTERM_GRACE: Duration = Duration::from_secs(2);

/// JSON-serializable input passed to a hook process via stdin.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookInput {
    pub event: String,
    pub session_dir: String,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Result of executing a single hook, including output, timing, and exit status.
#[derive(Debug)]
pub(crate) struct HookExecResult {
    pub output: HookOutput,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub stderr: String,
    pub timed_out: bool,
    pub package_name: String,
}

/// Execute a sequence of hooks, chaining outputs and respecting matchers.
pub async fn execute_hooks(
    hooks: &[HookInstance],
    input: &HookInput,
    session_dir: &Path,
    workspace: &Path,
) -> Result<Vec<HookExecResult>> {
    let mut results = Vec::new();
    let mut merged = HookOutput::default();

    for (index, hook) in hooks.iter().enumerate() {
        let mut hook_input = input.clone();

        if hook.chain && !results.is_empty() {
            let chain_value = serde_json::to_value(&merged).unwrap_or_default();
            if let serde_json::Value::Object(ref mut map) = hook_input.extra {
                map.insert("_chain".to_string(), chain_value);
            }
        }

        if let Some(ref matcher) = hook.matcher {
            let vars = build_matcher_vars(&hook_input);
            if !super::matcher::evaluate(matcher, &vars) {
                continue;
            }
        }

        let result = execute_single_hook(hook, &hook_input, session_dir, workspace, index).await?;

        merged = merge_outputs(&[merged, result.output.clone()]);

        results.push(result);
    }

    Ok(results)
}

async fn execute_single_hook(
    hook: &HookInstance,
    input: &HookInput,
    session_dir: &Path,
    workspace: &Path,
    index: usize,
) -> Result<HookExecResult> {
    let stdin_json = serde_json::to_string(input).map_err(|e| {
        Error::PluginHookSpawn(
            hook.command.clone(),
            std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        )
    })?;

    let mut cmd = tokio::process::Command::new(&hook.command);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd.env_clear();
    cmd.env("KUKU_SESSION_DIR", session_dir);
    cmd.env("KUKU_PACKAGE_DIR", &hook.package_root);
    cmd.env("KUKU_WORKSPACE", workspace);
    for var in ["PATH", "HOME", "USERPROFILE", "LANG", "LC_ALL"] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
    for var_name in &hook.env {
        if let Ok(val) = std::env::var(var_name) {
            cmd.env(var_name, val);
        }
    }

    let start = std::time::Instant::now();

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::PluginHookSpawn(hook.command.clone(), e))?;

    if let Some(ref mut stdin) = child.stdin {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(stdin_json.as_bytes()).await;
    }
    drop(child.stdin.take());

    let mut child_stdout = child.stdout.take();
    let mut child_stderr = child.stderr.take();

    let timed_out;
    let status = match tokio::time::timeout(hook.timeout, child.wait()).await {
        Ok(Ok(status)) => {
            timed_out = false;
            status
        }
        Ok(Err(e)) => {
            return Err(Error::PluginHookSpawn(hook.command.clone(), e));
        }
        Err(_) => {
            let _ = child.start_kill();
            tokio::time::sleep(SIGTERM_GRACE).await;
            let _ = child.kill().await;
            let duration = start.elapsed().as_millis() as u64;
            return Ok(HookExecResult {
                output: HookOutput::default(),
                exit_code: -1,
                duration_ms: duration,
                stderr: "hook timed out".to_string(),
                timed_out: true,
                package_name: hook.package_name.clone(),
            });
        }
    };

    let duration = start.elapsed().as_millis() as u64;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    if let Some(ref mut s) = child_stdout {
        use tokio::io::AsyncReadExt;
        let _ = s.read_to_end(&mut stdout_bytes).await;
    }
    if let Some(ref mut s) = child_stderr {
        use tokio::io::AsyncReadExt;
        let _ = s.read_to_end(&mut stderr_bytes).await;
    }
    let stdout_raw = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
    let exit_code = status.code().unwrap_or(-1);

    let stdout_handled = handle_overflow(&stdout_raw, session_dir, index);
    let hook_output = if exit_code == 2 {
        HookOutput {
            block: true,
            ..Default::default()
        }
    } else {
        parse_stdout(&stdout_handled)
    };

    Ok(HookExecResult {
        output: hook_output,
        exit_code,
        duration_ms: duration,
        stderr,
        timed_out,
        package_name: hook.package_name.clone(),
    })
}

fn json_to_string(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn build_matcher_vars(input: &HookInput) -> std::collections::HashMap<String, String> {
    let mut vars = std::collections::HashMap::new();
    vars.insert("event".to_string(), input.event.clone());

    if let serde_json::Value::Object(ref map) = input.extra {
        for (key, value) in map {
            if key == "_chain" {
                continue;
            }
            vars.insert(key.clone(), json_to_string(value));
            if key == "args" {
                if let serde_json::Value::Object(ref args_map) = value {
                    for (arg_key, arg_val) in args_map {
                        vars.insert(format!("args.{arg_key}"), json_to_string(arg_val));
                    }
                }
            }
        }
    }

    vars
}
