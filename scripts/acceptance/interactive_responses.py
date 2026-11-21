"""Exercise public and opaque Responses reasoning through the installed client."""

import json
import re
from urllib.parse import urlencode

from interactive_fixture import InstalledClient, Response, StreamPause


def event(kind, **fields):
    return b"data: " + json.dumps({"type": kind, **fields}).encode() + b"\n\n"


def completed(items, reasoning_tokens):
    return event("response.completed", response={
        "status": "completed", "output": items, "usage": {
            "input_tokens": 54, "output_tokens": 48,
            "output_tokens_details": {"reasoning_tokens": reasoning_tokens},
        },
    })


def responses_reasoning(binary, bridge, directory):
    client = InstalledClient(binary, bridge, directory)
    dispatch = client.provider.dispatch

    def responses(method, path, headers, body):
        assert path == "/api/v1/responses", path
        return dispatch(method, "/v1/messages", headers, body)

    client.provider.dispatch = responses
    with client:
        configure_responses(client)
        workspace = client.workspace("work-responses-reasoning")
        prompt = "Review the synthetic Responses fixture"
        with client.terminal(workspace) as terminal:
            check_responses(client, workspace, terminal, prompt)
            terminal.exit_cleanly()
        client.provider.assert_idle()


def configure_responses(client):
    config = client.home / "config.toml"
    config.write_text(re.sub(r'^(base_url = "[^"]+)"$', r'\1/api/v1"', config.read_text(), flags=re.M)
                      .replace('format = "anthropic"', 'format = "openai-responses"')
                      .replace('profile = "compatible"', 'profile = "openrouter"')
                      .replace('claude-sonnet-4-6', 'google/gemini-4-neon')
                      .replace('think = "off"', 'think = "medium"')
                      .replace('reasoning_efforts = ["none"]', 'reasoning_efforts = ["medium"]'),
                      encoding="utf-8")


def check_responses(client, workspace, terminal, prompt):
    tool_preview = "检查 seed.txt 文件"
    final_preview = "Finish checking the seed file."
    published_summary = "Provider checked the file result."
    opaque = "fixture-opaque-context"
    hidden = "fixture-signature"
    terminal.expect("New session", state="Ready")
    terminal.submit(prompt)
    first = client.provider.next()
    assert prompt in json.dumps(first.request["input"])
    assert first.request["reasoning"] == {"effort": "medium", "summary": "auto"}
    opaque_item = {"id": "rs_opaque", "type": "reasoning", "summary": [], "encrypted_content": opaque}
    readable = {"id": "rs_tool", "type": "reasoning", "summary": [],
                "content": [{"type": "reasoning_text", "text": tool_preview}]}
    tool = {"id": "fn_find", "type": "function_call", "call_id": "call_find", "name": "find_files",
            "arguments": json.dumps({"path": "."})}
    (workspace / "seed.txt").write_text("synthetic content", encoding="utf-8")
    start = event("response.output_item.added", output_index=0, item=opaque_item)
    public = event("response.output_item.added", output_index=1, item={**readable, "content": []})
    public += event("response.reasoning_text.delta", output_index=1, item_id="rs_tool",
                    content_index=0, delta=tool_preview)
    public += event("response.reasoning_text.done", output_index=1, item_id="rs_tool",
                    content_index=0, text=tool_preview)
    public += event("response.output_item.done", output_index=1, item=readable)
    completion = event("response.output_item.added", output_index=2, item={**tool, "arguments": ""})
    completion += event("response.function_call_arguments.delta", output_index=2, item_id=tool["id"],
                        delta=tool["arguments"])
    completion += event("response.output_item.done", output_index=2, item=tool)
    completion += completed([opaque_item, readable, tool], 29)
    waiting, streamed = StreamPause(), StreamPause()
    first.response.put_nowait(Response(200, start + public + completion, "text/event-stream",
                                      ((len(start), waiting), (len(start + public), streamed))))
    try:
        assert waiting.arrived.wait(8)
        terminal.expect("Thinking", absent=("View thinking", opaque))
        waiting.release.set()
        assert streamed.arrived.wait(8)
        terminal.expect(tool_preview, also=("View thinking",), absent=(opaque,))
    finally:
        waiting.release.set()
        streamed.release.set()
    second = client.provider.next()
    terminal.expect("View thinking ›", also=("Find files",), absent=(opaque, hidden))
    native = second.request["input"]
    assert opaque_item in native and readable in native and tool in native
    assert any(item.get("call_id") == "call_find" and item.get("type") == "function_call_output"
               and "seed.txt" in item["output"] for item in native)
    answer = "The seed file was found."
    output = [
        {"id": "rs_final", "type": "reasoning", "summary": [], "encrypted_content": opaque,
         "content": [{"type": "reasoning_text", "text": final_preview}, {"type": "signature", "text": hidden}]},
        {"id": "rs_summary", "type": "reasoning", "summary": [{"type": "summary_text", "text": published_summary}],
         "content": [{"type": "reasoning_text", "text": "Alternate summary body."}]},
        {"id": "msg_final", "type": "message", "role": "assistant",
         "content": [{"type": "output_text", "text": answer}]},
    ]
    partial = event("response.output_text.delta", delta=answer)
    before_usage = StreamPause()
    second.response.put_nowait(Response(200, partial + completed(output, 41), "text/event-stream",
                                       ((len(partial), before_usage),)))
    try:
        assert before_usage.arrived.wait(8)
        terminal.expect(answer, state="Reply", also=(f"~{(len(answer.encode()) + 3) // 4} tok",))
    finally:
        before_usage.release.set()
    screen = terminal.expect(answer, state="Finished", also=("View thinking", "48 tok"), absent=(opaque, hidden))["screen"]
    before_reader, mark, after_reader = screen.rpartition("View thinking")
    terminal.click_text(before_reader.replace("View thinking", "             ") + mark + after_reader,
                        "View thinking")
    terminal.expect(final_preview, also=(published_summary, answer), absent=(opaque, hidden, "Alternate summary body."))
    session = client.session(workspace)
    query = urlencode({"workspace": workspace, "agent": "main", "limit": 30})
    history = client.request("GET", f"/sessions/{session}/history?{query}").as_json()["entries"]
    published = [(item["text"], item["reasoning_preview"]) for item in history if "reasoning_preview" in item]
    assert published == [("", tool_preview), (answer, final_preview + "\n\n" + published_summary)], published
    assert client.snapshot(workspace, session)["usage"]["output_tokens"] == 96
    print("Responses reasoning streamed, survived tool continuation, and appeared in the final reader.")
