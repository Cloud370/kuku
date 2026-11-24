use serde::Deserialize;
use serde_json::{Value, json};

use crate::execution::{CommandTermination, OutputReference, WorkspaceActionDetails};
use crate::tool::builtin::run_command::output::validate_output_id;

use super::host::{bounded_json, cap_str};
use super::{ScriptOperation, WorkspaceScriptActionFacts, WorkspaceScriptActionStatus};

const MAX_RECOVERY_ROWS: usize = 4;
const MAX_RECOVERY_ROW_BYTES: usize = 1024;
const MAX_PROGRAM_LABEL_BYTES: usize = 128;

pub(crate) fn append_recovery_evidence(
    content: &mut String,
    actions: &[WorkspaceScriptActionFacts],
    can_read_output: bool,
) {
    let mut rows = Vec::with_capacity(MAX_RECOVERY_ROWS);
    let mut remaining = MAX_RECOVERY_ROW_BYTES;
    let mut omitted = 0;
    let mut has_output = false;
    for action in actions.iter().rev() {
        let Some(row) = recovery_row(action) else {
            continue;
        };
        if rows.len() == MAX_RECOVERY_ROWS {
            omitted += 1;
            continue;
        }
        match bounded_json(&row, remaining.saturating_sub(1)) {
            Ok(bytes) => {
                remaining -= bytes.len() + 1;
                has_output |= row.get("output_ref").is_some();
                rows.push(bytes);
            }
            Err(_) => omitted += 1,
        }
    }
    if !rows.is_empty() {
        content.push_str("\nObserved host results (action IDs follow request order):");
        for bytes in rows.iter().rev() {
            content.push('\n');
            content.push_str(std::str::from_utf8(bytes).expect("JSON is UTF-8"));
        }
    }
    if omitted != 0 {
        content.push_str(&format!(
            "\nAdditional observed results omitted: {omitted}."
        ));
    }
    if can_read_output && has_output {
        content.push_str(
            "\nInspect retained bytes with read_output({id, offset, limit}) using output_ref.",
        );
    }
}

fn recovery_row(action: &WorkspaceScriptActionFacts) -> Option<Value> {
    use ScriptOperation as Op;
    let ok = action.status == WorkspaceScriptActionStatus::Ok;
    let mut row = json!({"action":action.id, "tool":action.tool});
    let facts = &action.result;
    match action.tool {
        Op::RunCommand => add_command(&mut row, facts)?,
        Op::WriteFile | Op::EditFile | Op::RemoveFile => {
            let details = WorkspaceActionDetails::deserialize(facts).ok()?;
            row["commit_state"] = json!(details.commit_state?);
            row["path"] = json!(facts["path"].as_str()?);
        }
        Op::FinishOutput if ok => {
            row["output_ref"] = json!(output_reference(&facts["output_ref"])?);
            row["capture_complete"] = facts["capture_complete"].clone();
        }
        Op::SaveValue if ok => {
            row["name"] = json!(facts["name"].as_str()?);
            row["output_ref"] = json!(output_reference(&facts["reference"])?);
        }
        Op::FinishOutput
        | Op::SaveValue
        | Op::FindFiles
        | Op::ReadFile
        | Op::SearchText
        | Op::StatFile
        | Op::ReadOutput
        | Op::SearchOutput
        | Op::ListValues
        | Op::CreateOutput
        | Op::AppendOutput => return None,
    }
    Some(row)
}

fn add_command(row: &mut Value, facts: &Value) -> Option<()> {
    let details = WorkspaceActionDetails::deserialize(facts).ok()?;
    let termination = details.termination?;
    match termination {
        CommandTermination::Exited
        | CommandTermination::ProgramMissing
        | CommandTermination::StartFailed => {}
        CommandTermination::TimedOut | CommandTermination::Cancelled
            if facts["started"] == true => {}
        CommandTermination::TimedOut | CommandTermination::Cancelled => return None,
    }
    row["termination"] = json!(termination);
    if let Some(program) = command_program(facts) {
        row["program"] = json!(program);
    }
    if matches!(
        termination,
        CommandTermination::ProgramMissing | CommandTermination::StartFailed
    ) {
        row["started"] = json!(false);
        return Some(());
    }
    if let Some(code) = details.exit_code {
        row["exit_code"] = json!(code);
    }
    if let Some(signal) = facts["signal"].as_i64() {
        row["signal"] = json!(signal);
    }
    if let Some(reference) = details
        .output_ref
        .filter(|reference| reference.bytes != 0 && validate_output_id(&reference.id).is_ok())
    {
        row["output_ref"] = json!(reference);
        if let Some(complete) = details.capture_complete {
            row["capture_complete"] = json!(complete);
        }
    }
    Some(())
}

pub(super) fn command_program(facts: &Value) -> Option<&str> {
    let program = facts["program"]
        .as_str()
        .or_else(|| facts["invocation"]["program"].as_str())?;
    Some(cap_str(program, MAX_PROGRAM_LABEL_BYTES)).filter(|program| !program.is_empty())
}

fn output_reference(value: &Value) -> Option<OutputReference> {
    let reference = OutputReference::deserialize(value).ok()?;
    validate_output_id(&reference.id).ok()?;
    Some(reference)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{MAX_RECOVERY_ROW_BYTES, append_recovery_evidence};
    use crate::{
        WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptOperation,
    };

    #[test]
    fn recovery_preserves_observed_paths_and_bounds_the_preview() {
        use WorkspaceScriptActionStatus::{Blocked, Error, Ok};
        use WorkspaceScriptOperation::{EditFile, RemoveFile, WriteFile};

        let make = |n, tool, status, result| WorkspaceScriptActionFacts {
            id: format!("action_{n}"),
            tool,
            status,
            summary: "preview must use structured facts".into(),
            truncated: false,
            result,
        };
        let committed = |path: &str| json!({"path":path,"commit_state":"committed"});
        let long_path = format!("candidate/{}", "长".repeat(500));
        let actions = [
            make(1, WriteFile, Ok, committed("old.txt")),
            make(2, WriteFile, Ok, committed("prior.txt")),
            make(3, WriteFile, Ok, committed("output\n\u{1b}.txt")),
            make(
                4,
                EditFile,
                Error,
                json!({"path":"uncertain.txt","commit_state":"published_unconfirmed"}),
            ),
            make(5, RemoveFile, Ok, committed("deleted.txt")),
            make(6, WriteFile, Blocked, json!({"path":"blocked.txt"})),
            make(7, WriteFile, Ok, committed(&long_path)),
            make(8, WriteFile, Blocked, Value::Null),
        ];
        let mut content = "script failed".to_owned();
        append_recovery_evidence(&mut content, &actions, false);
        let decoded: Vec<Value> = content
            .lines()
            .filter(|line| line.starts_with('{'))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(4, decoded.len(), "{content}");
        assert_eq!("prior.txt", decoded[0]["path"]);
        assert_eq!("output\n\u{1b}.txt", decoded[1]["path"]);
        assert_eq!("published_unconfirmed", decoded[2]["commit_state"]);
        assert_eq!("remove_file", decoded[3]["tool"]);
        assert!(content.contains("Additional observed results omitted: 2."));
        assert!(!content.contains("blocked.txt"));
        assert!(!content.contains("old.txt"));
        assert!(content.len() < MAX_RECOVERY_ROW_BYTES + 128);
    }
}
