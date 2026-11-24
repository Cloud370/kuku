use std::io::{self, Write};

use crate::error::{Error, Result};

use super::{EventPayload, MAX_RECORD_BYTES, StoredEvent};

const MAX_RECORD_DEPTH: usize = 128;

pub(super) fn serialize_record(event: &StoredEvent) -> Result<Vec<u8>> {
    let mut probe = RecordWriter::new(None);
    let checked = probe_payload(&event.payload, &mut probe);
    probe.finish(checked)?;

    let mut record = RecordWriter::new(Some(Vec::new()));
    let serialized = serde_json::to_writer(&mut record, event);
    record.finish(serialized)?;
    let mut bytes = record.bytes.unwrap();
    serde_json::from_slice::<StoredEvent>(&bytes).map_err(|error| {
        Error::InvalidArgument(format!("event record cannot be replayed: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

macro_rules! probe_fields {
    ($payload:expr, $writer:expr, $( $variant:ident { $( $field:ident ),* } ),* $(,)?) => {
        match $payload {
            $(EventPayload::$variant { $( $field ),* } => {
                $(serde_json::to_writer(&mut *$writer, $field)?;)*
                Ok(())
            },)*
            EventPayload::Unknown(_) => Ok(()),
        }
    };
}

fn probe_payload(payload: &EventPayload, writer: &mut RecordWriter) -> serde_json::Result<()> {
    // Probe borrowed fields before generic JSON decoding allocates owned values.
    probe_fields!(
        payload,
        writer,
        ContextSources {
            turn,
            ts,
            request_id,
            project_instruction_sources,
            memory_sources
        },
        ContextSkills {
            conversation,
            turn,
            ts,
            registry,
            bootstrap_loaded
        },
        ModelResponse {
            conversation,
            turn,
            ts,
            request_id,
            text,
            thinking,
            stop_reason,
            input_tokens_total,
            output_tokens_total
        },
        ModelError {
            conversation,
            turn,
            ts,
            request_id,
            kind,
            message
        },
        ModelRecovery {
            conversation,
            turn,
            ts,
            from_request_id,
            to_request_id,
            reason,
            attempt,
            max_attempts,
            failed_max_output_tokens,
            retry_max_output_tokens,
            output_tokens_total,
            discarded_tool_calls,
            notice,
            prompt_path,
            prompt_hash
        },
        ToolCall {
            turn,
            ts,
            conversation,
            tool_call_id,
            request_id,
            index,
            tool,
            args
        },
        PermissionAllow {
            turn,
            ts,
            tool_call_id,
            tool,
            scope,
            matcher,
            source
        },
        PermissionRequested {
            turn,
            ts,
            tool_call_id,
            tool,
            risk,
            summary,
            candidate,
            source,
            workspace
        },
        PermissionDeny {
            turn,
            ts,
            tool_call_id,
            tool,
            reason,
            source
        },
        ToolResult {
            turn,
            ts,
            conversation,
            tool_call_id,
            status,
            summary,
            model_content,
            truncated,
            files_read,
            files_changed,
            commands_run,
            memory_changed,
            structured
        },
        Handoff {
            turn,
            ts,
            request_id,
            summary,
            keep_turns
        },
        SessionCreated {
            ts,
            schema_version,
            session_id,
            created_at,
            kuku_version,
            default_model
        },
        PromptSnapshot {
            ts,
            conversation,
            snapshot_id,
            turn,
            messages,
            project_instruction_sources,
            memory_sources,
            prompt_asset_sources,
            skills,
            bootstrap_loaded,
            provider,
            model,
            renderer,
            tool_registry,
            agent_registry,
            skill_registry,
            plugin_registry,
            capabilities
        },
        MessageUser {
            ts,
            conversation,
            turn,
            text,
            from,
            job_delivery,
            images
        },
        MessageAssistant {
            ts,
            conversation,
            turn,
            message_id,
            text
        },
        TurnStarted {
            ts,
            conversation,
            turn
        },
        TurnCompleted {
            ts,
            conversation,
            turn
        },
        TurnCancelled {
            ts,
            conversation,
            turn,
            reason,
            stop_event_id
        },
        TurnInterrupted {
            ts,
            conversation,
            turn,
            reason,
            partial_reply
        },
    )
}

struct RecordWriter {
    bytes: Option<Vec<u8>>,
    written: usize,
    depth: usize,
    string: StringState,
    violation: Option<RecordViolation>,
}

impl RecordWriter {
    fn new(bytes: Option<Vec<u8>>) -> Self {
        Self {
            bytes,
            written: 0,
            depth: 0,
            string: StringState::Outside,
            violation: None,
        }
    }

    fn finish(&self, result: serde_json::Result<()>) -> Result<()> {
        match self.violation {
            Some(RecordViolation::Bytes) => Err(Error::InvalidArgument(format!(
                "event record exceeds {MAX_RECORD_BYTES} bytes"
            ))),
            Some(RecordViolation::Depth) => Err(Error::InvalidArgument(format!(
                "event record exceeds {MAX_RECORD_DEPTH} nesting levels"
            ))),
            None => result.map_err(Error::from),
        }
    }

    fn inspect(&mut self, buffer: &[u8]) -> io::Result<()> {
        if buffer.len() > MAX_RECORD_BYTES - 1 - self.written {
            return self.reject(RecordViolation::Bytes);
        }
        for byte in buffer {
            match self.string {
                StringState::Escape => self.string = StringState::Inside,
                StringState::Inside => match byte {
                    b'\\' => self.string = StringState::Escape,
                    b'"' => self.string = StringState::Outside,
                    _ => {}
                },
                StringState::Outside => match byte {
                    b'"' => self.string = StringState::Inside,
                    b'{' | b'[' => {
                        if self.depth == MAX_RECORD_DEPTH {
                            return self.reject(RecordViolation::Depth);
                        }
                        self.depth += 1;
                    }
                    b'}' | b']' => self.depth -= 1,
                    _ => {}
                },
            }
        }
        self.written += buffer.len();
        Ok(())
    }

    fn reject(&mut self, violation: RecordViolation) -> io::Result<()> {
        self.violation = Some(violation);
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "event record limit exceeded",
        ))
    }
}

impl Write for RecordWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inspect(buffer)?;
        if let Some(bytes) = &mut self.bytes {
            bytes.extend_from_slice(buffer);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum StringState {
    Outside,
    Inside,
    Escape,
}

#[derive(Clone, Copy)]
enum RecordViolation {
    Bytes,
    Depth,
}
