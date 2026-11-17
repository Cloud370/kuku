"""Browser continuation against the installed binary's same-origin assets."""

import contextlib
import re
from urllib.parse import urlsplit

from playwright.sync_api import Error, expect, sync_playwright

from interactive_fixture import TOOL_OUTPUT, eventually


def launch_url(client, workspace, session):
    stdout, _ = client.cli(workspace, "web", "--print-url", "--session", session, "--agent", "main")
    url = stdout.strip()
    parts = urlsplit(url)
    assert f"{parts.scheme}://{parts.netloc}" == f"http://{client.connection()['address']}"
    assert parts.fragment.startswith("ticket="), "Browser launcher did not issue a single-use ticket"
    assert client.connection()["token"] not in url, "Browser URL contains the managed service bearer token"
    return url


@contextlib.contextmanager
def open_session(browser, client, workspace, session):
    context = browser.new_context()
    try:
        context.set_default_timeout(15_000)
        url = launch_url(client, workspace, session)
        address = f"http://{client.connection()['address']}"
        unauthenticated = context.request.get(f"{address}/service/status")
        assert unauthenticated.status == 401, "A fresh browser must authenticate"
        page = context.new_page()
        try:
            response = page.goto(url, wait_until="domcontentloaded")
            assert response and response.ok, "Installed binary did not serve its WebUI"
            page.wait_for_function("location.hash === ''")
        except Error:
            raise AssertionError("Browser could not open the installed WebUI and consume its bootstrap ticket") from None
        yield page
    finally:
        context.close()


def browser_handoff(client):
    workspace = client.workspace("work-browser")
    prompt = "continue in the installed browser"
    question = "Which browser choice should I apply?"
    answer = "answer from browser"
    tool_id = "browser-gated-command"
    final = "browser task completed once"
    with client.terminal(workspace) as terminal:
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        client.provider.next(prompt).tool("ask_user", "browser-waiting-fixture", question=question)
        terminal.expect(question)
        terminal.exit_cleanly()
    session = client.session(workspace)
    connection = client.connection()

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        try:
            with open_session(browser, client, workspace, session) as page:
                expect(page.get_by_text(question, exact=True).first).to_be_visible()
                page.get_by_role("textbox", name="Answer", exact=True).fill(answer)
                page.get_by_role("button", name="Send answer", exact=True).click()
                client.gated_tool(workspace, client.provider.next(answer), tool_id)

            client.assert_current_service(connection)
            client.release_tool(workspace)
            client.provider.next(tool_id).text(final)
            eventually("server completion with terminal and browser closed",
                       lambda: client.snapshot(workspace, session)["last_output"] == final)
            client.verify_tool(workspace)

            with open_session(browser, client, workspace, session) as page:
                result = page.get_by_text(final, exact=True)
                expect(result).to_have_count(1)
                expect(result).to_be_visible()
                page.get_by_role("button", name=re.compile("run_command")).first.click()
                page.get_by_role("button", name="Load output", exact=True).click()
                expect(page.get_by_text(TOOL_OUTPUT, exact=False).first).to_be_visible()
        finally:
            browser.close()

    with client.terminal(workspace) as terminal:
        terminal.resume(prompt)
        assert terminal.expect(final)["screen"].count(final) == 1
        terminal.exit_cleanly()
    client.assert_current_service(connection)
    client.provider.assert_idle(duration=0.2)
    print("Installed WebUI authenticated, answered a terminal question, and recovered one result after disconnect")
