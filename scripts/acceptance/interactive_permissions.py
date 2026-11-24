"""Approve an installed client's workspace operations and observe terminal bells."""

import json
import os
import tempfile
from pathlib import Path
from urllib.parse import urlencode

from interactive_fixture import eventually


F2 = "\x1bOQ"
F3 = "\x1bOR"
DOWN = "\x1b[B"


def workspace_permissions(client):
    workspace = client.workspace("work-permissions")
    (workspace / "input.txt").write_text("initial content", encoding="utf-8")
    (client.home / "tui.toml").write_text(
        '[statusline]\nfields = ["permission_mode"]\n', encoding="utf-8"
    )
    with tempfile.TemporaryDirectory() as outside:
        exercise_permissions(client, workspace, Path(outside).resolve())
    restart_pending_script(client, workspace)


def exercise_permissions(client, workspace, outside):
    title = "Observe a script while it waits"
    query = urlencode({"workspace": workspace})
    path = f"/workspaces/permissions?{query}"
    with client.terminal(workspace, confirm_permissions=False) as terminal:
        terminal.expect("Workspace permissions", also=("› Full access", "Read-only"))
        terminal.press(DOWN + "\r")
        terminal.expect("Permissions · Ask", state="Ready")
        terminal.submit(title)
        call = client.provider.next(title)
        assert "Current workspace permission mode: Ask." in json.dumps(call.request)
        outside_script = outside / "script.txt"
        code = (
            'const input = await tools.read_file({path:"input.txt"});'
            'if(input.status!=="ok") throw new Error(input.error);'
            f'const write = await tools.write_file({{path:{json.dumps(str(outside_script))},content:input.result.raw_text}});'
            'if(write.status!=="ok") throw new Error(write.error);'
            'const last = await tools.write_file({path:"after.txt",content:"continued"});'
            'return last.status;'
        )
        call.tool("run_js", "approval-script", code=code, background=True)
        terminal.expect("Tool approvals (1) · F2 Review")
        assert not outside_script.exists() and not (workspace / "after.txt").exists()
        job_call = client.provider.next("approval-script")
        job = json.loads(job_call.tool_result("approval-script")["content"])
        assert job["kind"] == "job", job
        job_call.text("The script is waiting in the background.")
        terminal.expect("The script is waiting in the background.")
        if os.name != "nt":
            assert terminal.request("screen")["audible_bells"] > 0
        draft = "preserve this unsent draft"
        terminal.type(draft)
        terminal.press("\x03")
        terminal.exit_cleanly()

    with client.terminal(workspace, confirm_permissions=False) as terminal:
        terminal.resume(title)
        terminal.expect("Tool approvals (1) · F2 Review")
        assert terminal.request("screen")["audible_bells"] == 0, "Replaying a pending request rang a bell"
        terminal.type(draft)
        terminal.press(F2)
        terminal.expect("Tool approval", also=("Ask mode", job["job_id"], "Trust this workspace and allow"))
        terminal.press("a")
        terminal.expect(draft, absent=("Tool approval",))
        eventually("the original script to resume at the approved operation",
                   lambda: (workspace / "after.txt").exists())
        assert outside_script.read_text(encoding="utf-8") == "initial content"
        assert (workspace / "after.txt").read_text(encoding="utf-8") == "continued"
        client.provider.next(job["job_id"]).text("The script continued and persisted its results.")
        terminal.expect("The script continued and persisted its results.")
        terminal.press("\x03")

        terminal.press(F3)
        terminal.expect("Workspace permissions", also=("Full access · current",))
        terminal.press("\x1b")
        terminal.expect("The script continued and persisted its results.", absent=("Workspace permissions",))
        direct = outside / "direct.txt"
        terminal.submit("Write to a separate directory")
        call = client.provider.next("Write to a separate directory")
        assert "Current workspace permission mode: Full." in json.dumps(call.request)
        call.tool("write_file", "outside-write", path=str(direct), content="direct write")
        client.provider.next("outside-write").text("The external file is ready.")
        terminal.expect("The external file is ready.")
        assert direct.read_text(encoding="utf-8") == "direct write"

        child_task = "Create a file from the worker's own directory"
        terminal.submit("Use an Agent with a separate workspace")
        client.provider.next("Use an Agent with a separate workspace").tool(
            "create_agent", "create-worker", address="worker", workspace=str(outside),
            tools=["write_file"],
        )
        client.provider.next("create-worker").tool(
            "send_message", "send-to-worker", to="worker", message=child_task
        )
        first, second = client.provider.next(), client.provider.next()
        child, parent = (first, second) if "send-to-worker" in json.dumps(second.request) else (second, first)
        child.expect(child_task).tool("write_file", "worker-write", path="child.txt", content="worker result")
        parent.text("The worker has its own directory.")
        client.provider.next("worker-write").text("The worker file is ready.")
        eventually("the delegated file to land in the Agent's directory", lambda: (outside / "child.txt").exists())
        assert (outside / "child.txt").read_text(encoding="utf-8") == "worker result"
        assert not (workspace / "child.txt").exists()
        terminal.expect("The worker has its own directory.")

        terminal.submit("/perm read-only")
        terminal.expect("Permissions · Read-only")
        terminal.submit("Test the stricter mode")
        client.provider.next("Test the stricter mode").tool(
            "write_file", "read-only-write", path=str(direct), content="rejected"
        )
        result = client.provider.next("read-only-write")
        assert "read-only mode" in result.tool_result("read-only-write", error=True)["content"]
        result.text("Write blocked by read-only mode.")
        terminal.expect("Write blocked by read-only mode.")
        assert direct.read_text(encoding="utf-8") == "direct write"
        assert client.request("GET", path).as_json() == "read-only"

        terminal.submit("/notify")
        terminal.expect("Terminal notifications", also=("Tool approval", "Turn completed"))
        terminal.press(DOWN + " \r")
        terminal.expect("Terminal notifications saved")
        terminal.submit("/perm ask")
        terminal.expect("Permissions · Ask")
        terminal.submit("Ask to replace the contents")
        call = client.provider.next("Ask to replace the contents")
        bells = terminal.request("screen")["audible_bells"]
        call.tool("write_file", "deny-write", path=str(direct), content="denied")
        terminal.expect("Tool approvals (1) · F2 Review")
        assert terminal.request("screen")["audible_bells"] == bells, "Disabled approval bell fired"
        terminal.press(F2)
        terminal.expect("Tool approval", also=("Allow once", "Deny",))
        terminal.press("n")
        result = client.provider.next("deny-write")
        assert "denied" in result.tool_result("deny-write", error=True)["content"]
        result.text("The existing file stays intact.")
        terminal.expect("The existing file stays intact.")
        assert direct.read_text(encoding="utf-8") == "direct write"
        terminal.exit_cleanly()

    print("Applied workspace modes and verified native tool and Agent directories, background approval, and terminal sound settings.")


def restart_pending_script(client, workspace):
    title = "Recover facts after a service interruption"
    with client.terminal(workspace, confirm_permissions=False) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(title)
        client.provider.next(title).tool(
            "run_js", "interrupted-script", code=(
                'await tools.write_file({path:"prefix.txt",content:"committed"});'
                'await tools.write_file({path:"pending.txt",content:"needs approval"});'
            ),
        )
        terminal.expect("Tool approvals (1) · F2 Review")
        terminal.press(F2)
        terminal.expect("Tool approval", also=("prefix.txt",))
        terminal.press("y")
        eventually("the approved first write", lambda: (workspace / "prefix.txt").exists())
        session = next(item["session_id"] for item in client.sessions(workspace) if item["title"] == title)
        query = urlencode({"workspace": workspace})
        path = f"/sessions/{session}/permissions?{query}"

        def next_request():
            return next((item["request"] for item in client.request("GET", path).as_json()["approvals"]
                         if item["request"]["candidate"] == "pending.txt"), None)

        pending = eventually("the script to wait on the second write", next_request)
        client.kill(client.connection())

    with client.terminal(workspace, confirm_permissions=False) as terminal:
        terminal.resume(title)
        terminal.expect("The service restarted while awaiting approval.", state="Interrupted")
        assert client.request("GET", path).as_json()["approvals"] == []
        client.cli(workspace, "reply", session, pending["id"], "--allow", success=False)
        client.provider.assert_idle(duration=0.2)
        assert (workspace / "prefix.txt").read_text(encoding="utf-8") == "committed"
        assert not (workspace / "pending.txt").exists()
        events = client.request("GET", f"/sessions/{session}/events?{query}").as_json()["events"]
        facts = [event["payload"] for event in events]
        assert sum(fact.get("kind") == "workspace.action" and fact.get("target") == "prefix.txt"
                   and fact.get("status") == "ok" for fact in facts) == 1
        records = [fact["record"] for fact in facts if fact.get("kind") == "runtime.operation"]
        assert any(record.get("operation") == "script_permission_ended"
                   and record.get("request_id") == pending["id"] for record in records)
        terminal.expect("F2 Continue")
        terminal.press(F2 + "\r")
        resumed = client.provider.next("interrupted-script")
        result = resumed.tool_result("interrupted-script", error=True)["content"]
        assert "Restart the remaining script steps" in result and "prefix.txt" in result, result
        resumed.text("The saved prefix is available for the next step.")
        terminal.expect("The saved prefix is available for the next step.")
        terminal.exit_cleanly()

    print("A service restart retained the completed write once and expired the suspended script approval.")
