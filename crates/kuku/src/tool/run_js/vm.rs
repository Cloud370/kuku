#[cfg(any(unix, windows))]
mod allocation;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::{
    CString as JsCString, Context, Ctx, Error, Exception, Function, Object, Promise, Runtime,
    String as JsString, TypedArray, Value as JsValue,
};
use serde_json::Value;
use tokio::sync::{mpsc, OwnedSemaphorePermit};

use crate::tool::builtin::common::ExecutorObservation;
use crate::tool::builtin::run_command::encoding::display_text;
use crate::tool::ToolDefinition;

use super::host::{bounded_json, cap_str};
use super::{ScriptExecution, ScriptOperation};

const MAX_EXCEPTION_BYTES: usize = 2048;
const MAX_METHOD_DEFINITION_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_LINE_BYTES: usize = 128 * 1024;
// NOTE: QuickJS's AsyncFunction constructor adds two lines before the strict-mode directive.
const SCRIPT_WRAPPER_LINES: usize = 3;
const TOOLS_INSTALLER: &str = include_str!("tools.js");
const CONSOLE_INSTALLER: &str = include_str!("console.js");

pub(super) struct Request {
    pub(super) ordinal: usize,
    pub(super) op: ScriptOperation,
    pub(super) json: Vec<u8>,
}

pub(super) struct Reply {
    pub(super) ordinal: usize,
    pub(super) response: Result<BufferedResponse, &'static str>,
}

pub(super) struct BufferedResponse {
    pub(super) json: Vec<u8>,
    pub(super) observation: Option<ExecutorObservation>,
    pub(super) _capacity: OwnedSemaphorePermit,
}

#[derive(Debug)]
pub(super) enum Failure {
    Error(String),
    TimedOut,
    Stopped,
}

struct JavaScriptBudget {
    started: Instant,
    limit: Duration,
    host_wait: Mutex<Duration>,
}

impl JavaScriptBudget {
    fn new(limit: Duration) -> Self {
        Self {
            started: Instant::now(),
            limit,
            host_wait: Mutex::new(Duration::ZERO),
        }
    }

    fn expired(&self, now: Instant) -> bool {
        let elapsed = now.duration_since(self.started);
        elapsed.saturating_sub(*self.host_wait.lock().unwrap()) >= self.limit
    }

    fn wait_for_reply(&self, replies: &Receiver<Reply>) -> Result<Reply, Failure> {
        let wait_started = Instant::now();
        if self.expired(wait_started) {
            return Err(Failure::TimedOut);
        }
        let reply = replies.recv().map_err(|_| Failure::Stopped);
        *self.host_wait.lock().unwrap() += wait_started.elapsed();
        reply
    }
}

pub(super) fn run(
    source: String,
    methods: Vec<ToolDefinition>,
    execution: Arc<ScriptExecution>,
    requests: mpsc::Sender<Request>,
    replies: Receiver<Reply>,
) -> Result<Value, Failure> {
    let budget = Arc::new(JavaScriptBudget::new(Duration::from_millis(
        execution.limits.timeout_ms,
    )));
    #[cfg(any(unix, windows))]
    let runtime = Runtime::new_with_alloc(allocation::ScriptAllocator);
    #[cfg(not(any(unix, windows)))]
    let runtime = Runtime::new();
    let runtime = runtime.map_err(runtime_error)?;
    runtime.set_memory_limit(execution.limits.heap_bytes);
    runtime.set_max_stack_size(execution.limits.stack_bytes);
    let control = execution.clone();
    let interrupt_budget = budget.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        control.is_cancelled() || interrupt_budget.expired(Instant::now())
    })));
    let context = Context::full(&runtime).map_err(runtime_error)?;
    context.with(|ctx| {
        Worker {
            ctx,
            execution,
            budget,
            pending: Rc::new(RefCell::new(Pending::default())),
            requests,
            replies,
        }
        .run(&source, methods)
    })
}

#[derive(Default)]
struct Pending<'js> {
    requests: HashMap<usize, Continuation<'js>>,
    bytes: usize,
    issued: usize,
    closed: bool,
}

struct Continuation<'js> {
    resolve: Function<'js>,
    reject: Function<'js>,
    bytes: usize,
}

struct Worker<'js> {
    ctx: Ctx<'js>,
    execution: Arc<ScriptExecution>,
    budget: Arc<JavaScriptBudget>,
    pending: Rc<RefCell<Pending<'js>>>,
    requests: mpsc::Sender<Request>,
    replies: Receiver<Reply>,
}

impl<'js> Worker<'js> {
    fn run(&self, source: &str, methods: Vec<ToolDefinition>) -> Result<Value, Failure> {
        self.check_running()?;
        let console = self.console().map_err(|error| self.describe(error))?;
        let result = self
            .start(source, methods)
            .and_then(|promise| self.complete(promise, source));
        self.pending.borrow_mut().closed = true;
        let result = result.and_then(|value| {
            if value.is_undefined() {
                console.call(()).map_err(|error| self.describe(error))
            } else {
                Ok(value)
            }
        });
        let result = result.and_then(|value| self.return_value(value, source));
        self.check_running()?;
        result
    }

    fn console(&self) -> rquickjs::Result<Function<'js>> {
        let install: Function = self.ctx.eval(CONSOLE_INSTALLER)?;
        install.call((self.execution.limits.result_bytes,))
    }

    fn start(
        &self,
        source: &str,
        definitions: Vec<ToolDefinition>,
    ) -> Result<Promise<'js>, Failure> {
        let tools = self
            .tools(definitions)
            .map_err(|error| self.describe(error))?;
        let constructor: Function = self
            .ctx
            .eval("(async function () {}).constructor")
            .map_err(|error| self.describe(error))?;
        let body = format!("\"use strict\";\n{source}");
        let script: Function = constructor
            .call(("tools", body))
            .map_err(|error| self.describe_at(error, Some(source)))?;
        script
            .call((tools,))
            .map_err(|error| self.describe_at(error, Some(source)))
    }

    fn tools(&self, definitions: Vec<ToolDefinition>) -> rquickjs::Result<Object<'js>> {
        let tools = Object::new(self.ctx.clone())?;
        tools.set_prototype(None)?;
        let mut available = Vec::new();
        for op in ScriptOperation::ALL {
            let Some(definition) = definitions
                .iter()
                .find(|definition| definition.name == op.name())
            else {
                continue;
            };
            let bridge = Bridge {
                op,
                execution: self.execution.clone(),
                budget: self.budget.clone(),
                pending: Rc::downgrade(&self.pending),
                requests: self.requests.clone(),
            };
            let call = Function::new(self.ctx.clone(), move |ctx, args| bridge.send(ctx, args))?;
            tools.set(op.name(), call)?;
            available.push(definition);
        }
        let definitions = bounded_json(&available, MAX_METHOD_DEFINITION_BYTES).map_err(|_| {
            Exception::throw_range(
                &self.ctx,
                "tools method definitions exceed their byte limit",
            )
        })?;
        let definitions = self.ctx.json_parse(definitions)?;
        let encode = Function::new(self.ctx.clone(), encode_text)?;
        let decode = Function::new(self.ctx.clone(), decode_line)?;
        let install: Function = self.ctx.eval(TOOLS_INSTALLER)?;
        install.call((
            tools,
            definitions,
            encode,
            decode,
            MAX_OUTPUT_LINE_BYTES,
            super::MAX_OUTPUT_TEXT_BYTES,
        ))
    }

    fn complete(&self, promise: Promise<'js>, source: &str) -> Result<JsValue<'js>, Failure> {
        loop {
            self.check_running()?;
            if let Some(result) = promise.result() {
                self.pending.borrow_mut().closed = true;
                let value = result.map_err(|error| self.describe_at(error, Some(source)))?;
                if !self.pending.borrow().requests.is_empty() {
                    return Err(Failure::Error(
                        "script returned with pending host operations; await every host call"
                            .into(),
                    ));
                }
                return Ok(value);
            }
            let received = match self.replies.try_recv() {
                Ok(reply) => {
                    self.resume(reply).map_err(|error| self.describe(error))?;
                    true
                }
                Err(TryRecvError::Empty) => false,
                Err(TryRecvError::Disconnected) => return Err(Failure::Stopped),
            };
            let ran_job = self.ctx.execute_pending_job();
            if received || ran_job {
                continue;
            }
            if self.pending.borrow().requests.is_empty() {
                return Err(Failure::Error(
                    "script is waiting on a promise with no host work or scheduled JavaScript jobs"
                        .into(),
                ));
            }
            let reply = self.budget.wait_for_reply(&self.replies)?;
            self.check_running()?;
            self.resume(reply).map_err(|error| self.describe(error))?;
        }
    }

    fn resume(&self, reply: Reply) -> rquickjs::Result<()> {
        let continuation = {
            let mut pending = self.pending.borrow_mut();
            let continuation = pending
                .requests
                .remove(&reply.ordinal)
                .ok_or(Error::Unknown)?;
            pending.bytes -= continuation.bytes;
            continuation
        };
        match reply.response {
            Ok(reply) => {
                let response = self.ctx.json_parse(reply.json)?;
                if let Some(observation) = reply.observation {
                    self.execution.remember_observation(observation);
                }
                continuation.resolve.call((response,))
            }
            Err(error) => {
                let exception = Exception::from_message(self.ctx.clone(), error)?;
                continuation.reject.call((exception,))
            }
        }
    }

    fn return_value(&self, value: JsValue<'js>, source: &str) -> Result<Value, Failure> {
        if value.is_undefined() {
            return Ok(Value::Null);
        }
        let json =
            json_bytes(&self.ctx, value).map_err(|error| self.describe_at(error, Some(source)))?;
        let max_bytes = self.execution.limits.result_bytes;
        if json.len() > max_bytes {
            return Err(Failure::Error(format!(
                "script result exceeds {max_bytes} bytes; return a small value or retain derived output"
            )));
        }
        serde_json::from_str(json.as_str())
            .map_err(|_| Failure::Error("script result cannot be represented as JSON".into()))
    }

    fn check_running(&self) -> Result<(), Failure> {
        if self.execution.is_cancelled() {
            return Err(Failure::Stopped);
        }
        if self.budget.expired(Instant::now()) {
            return Err(Failure::TimedOut);
        }
        Ok(())
    }

    fn describe(&self, error: Error) -> Failure {
        self.describe_at(error, None)
    }

    fn describe_at(&self, error: Error, source: Option<&str>) -> Failure {
        self.pending.borrow_mut().closed = true;
        if let Err(stopped) = self.check_running() {
            return stopped;
        }
        if !matches!(error, Error::Exception) {
            return runtime_error(error);
        }
        let exception = self.ctx.catch();
        let location = source
            .zip(exception.as_exception())
            .and_then(|(source, error)| script_location(source, error))
            .map(|location| format!(" ({location})"));
        let message_budget = MAX_EXCEPTION_BYTES - location.as_ref().map_or(0, String::len);
        let message = if let Some(error) = exception.as_exception() {
            error.as_object().get("message").ok()
        } else {
            exception.is_string().then_some(exception)
        };
        let message = message
            .filter(JsValue::is_string)
            .and_then(|value| json_bytes(&self.ctx, value).ok())
            .map(|json| {
                if json.len() > message_budget {
                    format!(
                        "{}…",
                        cap_str(json.as_str().trim_start_matches('"'), message_budget)
                    )
                } else {
                    serde_json::from_str(json.as_str())
                        .unwrap_or_else(|_| "JavaScript exception".into())
                }
            });
        let mut message = message.unwrap_or_else(|| "JavaScript threw an exception".into());
        if let Some(location) = location {
            message.push_str(&location);
        }
        Failure::Error(message)
    }
}

fn script_location(source: &str, error: &Exception<'_>) -> Option<String> {
    let stack = error.stack()?;
    let (line, column) = stack.lines().find_map(|frame| {
        let frame = frame.trim_start().strip_prefix("at ")?;
        let coordinates = frame.strip_prefix("<input>:").or_else(|| {
            let (_, coordinates) = frame.rsplit_once(" (<input>:")?;
            coordinates.strip_suffix(')')
        })?;
        let (line, column) = coordinates.split_once(':')?;
        let line = line
            .parse::<usize>()
            .ok()?
            .checked_sub(SCRIPT_WRAPPER_LINES)?;
        let column = column.parse::<usize>().ok()?;
        (line > 0 && column > 0).then_some((line, column))
    })?;
    let lines =
        source.split(['\n', '\r', '\u{2028}', '\u{2029}']).count() - source.matches("\r\n").count();
    if line > lines {
        return Some("at the end of the script".into());
    }
    Some(format!("script line {line}, column {column}"))
}

struct Bridge<'js> {
    op: ScriptOperation,
    execution: Arc<ScriptExecution>,
    budget: Arc<JavaScriptBudget>,
    pending: Weak<RefCell<Pending<'js>>>,
    requests: mpsc::Sender<Request>,
}

impl<'js> Bridge<'js> {
    fn send(&self, ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<Promise<'js>> {
        let Some(state) = self.pending.upgrade() else {
            return Err(Exception::throw_message(
                &ctx,
                "workspace script has finished",
            ));
        };
        self.check_admission(&ctx, &state.borrow())?;
        if !args.is_object() || args.is_array() {
            return Err(Exception::throw_type(
                &ctx,
                "workspace methods require an object argument",
            ));
        }
        let json = json_bytes(&ctx, args)?;
        let mut pending = state.borrow_mut();
        self.check_admission(&ctx, &pending)?;
        if json.len() > self.execution.limits.host_bytes {
            return Err(Exception::throw_range(
                &ctx,
                "workspace host request exceeds its byte limit",
            ));
        }
        if json.len() > self.execution.limits.queued_bytes - pending.bytes {
            return Err(Exception::throw_range(
                &ctx,
                "workspace host request queue exceeds its byte limit",
            ));
        }
        let permit = self.requests.try_reserve().map_err(|_| {
            Exception::throw_message(&ctx, "workspace script request queue is unavailable")
        })?;
        let (promise, resolve, reject) = ctx.promise()?;
        pending.issued += 1;
        pending.bytes += json.len();
        let ordinal = pending.issued;
        pending.requests.insert(
            ordinal,
            Continuation {
                resolve,
                reject,
                bytes: json.len(),
            },
        );
        self.execution.accepted.store(ordinal, Ordering::Release);
        permit.send(Request {
            ordinal,
            op: self.op,
            json: json.as_str().as_bytes().to_vec(),
        });
        Ok(promise)
    }

    fn check_admission(&self, ctx: &Ctx<'js>, pending: &Pending<'js>) -> rquickjs::Result<()> {
        if pending.closed || self.execution.is_cancelled() || self.budget.expired(Instant::now()) {
            return Err(Exception::throw_message(
                ctx,
                "workspace script has stopped",
            ));
        }
        if pending.issued >= self.execution.limits.actions {
            return Err(Exception::throw_range(
                ctx,
                "workspace script action limit reached",
            ));
        }
        if pending.requests.len() >= self.execution.limits.queued_operations {
            return Err(Exception::throw_range(
                ctx,
                "workspace script host queue limit reached",
            ));
        }
        Ok(())
    }
}

fn json_bytes<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<JsCString<'js>> {
    ctx.json_stringify(value)?
        .ok_or_else(|| {
            Exception::throw_type(ctx, "workspace script value is not JSON-serializable")
        })?
        .to_cstring()
}

fn runtime_error(error: Error) -> Failure {
    Failure::Error(format!("JavaScript runtime: {error}"))
}

fn encode_text<'js>(ctx: Ctx<'js>, text: JsString<'js>) -> rquickjs::Result<TypedArray<'js, u8>> {
    TypedArray::new_copy(ctx, text.to_string()?.as_bytes())
}

fn decode_line<'js>(
    ctx: Ctx<'js>,
    bytes: TypedArray<'js, u8>,
    starts_line: bool,
    ends_stream: bool,
) -> rquickjs::Result<Object<'js>> {
    let (text, lossy_utf8) = {
        // SAFETY: This worker owns the runtime and executes no JavaScript until
        // decoding has copied the borrowed backing store into a Rust string.
        // rquickjs has no safe borrowed access to TypedArray elements.
        let bytes = unsafe { bytes.as_bytes() }.ok_or(Error::Unknown)?;
        let (text, lossy_utf8) = display_text(bytes, starts_line, ends_stream);
        (text.into_owned(), lossy_utf8)
    };
    let line = Object::new(ctx)?;
    line.set("text", text)?;
    line.set("lossy_utf8", lossy_utf8)?;
    Ok(line)
}
