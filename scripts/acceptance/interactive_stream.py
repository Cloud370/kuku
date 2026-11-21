"""Measure a controlled Markdown replay through the installed native terminal."""

import hashlib
import json
import math
import re
import time

from interactive_fixture import TIMEOUT


PREVIEW = "首段正在逐字显示"
DRAFT = "kept while CJK arrives"
LAST = "Stream profile complete"
MARKER = re.compile(r"Checkpoint \d{3}")
CJK = "逐项检查工作区结果，保留每项测试的结论，并核对后续变更与记录一致。"


def paragraph(number):
    return f"Checkpoint {number:03d} · {CJK * 6}\n\n"


def measure_terminal(terminal, label, predicate, since):
    deadline = time.monotonic() + TIMEOUT
    while True:
        view = terminal.request("screen")
        screen = view["screen"]
        if predicate(screen):
            return screen, round((time.perf_counter_ns() - since) / 1_000_000, 3)
        assert view["exit_code"] is None, f"Terminal exited while waiting for {label}"
        if time.monotonic() >= deadline:
            raise AssertionError(f"Timed out waiting for {label}\n{screen}")
        time.sleep(0.003)


def stats(samples):
    ordered = sorted(samples)
    return {"samples_ms": samples, "p95_ms": ordered[math.ceil(len(ordered) * 0.95) - 1]}


def profile_markdown_stream(client):
    workspace = client.workspace("work-stream-profile")
    prompt = "Replay the long streaming Markdown fixture"
    prefix = "".join(map(paragraph, range(42)))
    samples = [paragraph(i) for i in range(42, 58)]
    burst = "".join(map(paragraph, range(58, 75)))
    following = "".join(map(paragraph, range(75, 81)))
    slow = [
        (PREVIEW, PREVIEW),
        ("。\n\n| 验证项 | 结果 |\n| --- |", "验证项"),
        (" --- |\n| 中文 | 通过 |\n\n", "通过"),
        ("```rust\nfn preview", "fn preview"),
        ("() { }\n```\n\n", "preview()"),
        (prefix, "Checkpoint 041"),
    ]
    parts = [text for text, _ in slow] + samples + [burst, following, LAST, "\n"]
    source = "".join(parts)
    source_bytes = source.encode("utf-8")
    assert len(source_bytes) < 64 * 1024
    timings = {120: [], 60: []}

    with client.terminal(workspace) as terminal:
        terminal.request("resize", rows=28, cols=120)
        terminal.expect("New session", state="Ready")
        terminal.submit(prompt)
        call = client.provider.next(prompt)
        with call.streaming_text(*parts, chunk_size=17) as pauses:
            for index, (_, marker) in enumerate(slow):
                assert pauses[index].arrived.wait(10), f"Slow phase {index} did not arrive"
                _, elapsed = measure_terminal(terminal, marker, lambda screen: marker in screen,
                                              pauses[index].flushed_at_ns)
                if index == 0:
                    first_visible = elapsed
                if index + 1 != len(slow):
                    time.sleep(0.06)
                    pauses[index].release.set()
            for offset, piece in enumerate(samples):
                index = len(slow) + offset
                width = 120 if offset < len(samples) // 2 else 60
                if terminal.cols != width:
                    terminal.request("resize", rows=28, cols=width)
                    terminal.expect("Reply", rows=28)
                marker = MARKER.match(piece)[0]
                pauses[index - 1].release.set()
                assert pauses[index].arrived.wait(10), f"Measured phase {index} did not arrive"
                _, elapsed = measure_terminal(terminal, marker, lambda screen: marker in screen,
                                              pauses[index].flushed_at_ns)
                timings[width].append(elapsed)

            index = len(slow) + len(samples)
            pauses[index - 1].release.set()
            echo_started = time.perf_counter_ns()
            terminal.press(DRAFT)
            _, echo_ms = measure_terminal(terminal, "composer echo", lambda screen: DRAFT in screen,
                                         echo_started)
            assert pauses[index].arrived.wait(10)
            terminal.expect("Checkpoint 074", also=(DRAFT,))
            terminal.press("\x1b[5~")
            reading = terminal.expect("Alt+↑", also=(DRAFT,))["screen"]
            anchor = next((row, match[0]) for row, line in enumerate(reading.splitlines())
                          if (match := MARKER.search(line)))
            pauses[index].release.set()
            assert pauses[index + 1].arrived.wait(10)
            pauses[index + 1].release.set()
            assert pauses[index + 2].arrived.wait(10)
            held = terminal.expect("Alt+↑", also=(DRAFT, "New"))["screen"]
            assert anchor[1] in held.splitlines()[anchor[0]], held
            pauses[index + 2].release.set()

        terminal.press("\x1b[1;5F")
        terminal.expect(LAST, state="Finished", also=(DRAFT,))
        snapshot = client.snapshot(workspace, client.session(workspace))
        assert snapshot["last_output"] == source, snapshot["status"]
        terminal.press("\x03")
        terminal.expect(LAST, absent=(DRAFT,))
        terminal.exit_cleanly()
    client.provider.assert_idle()
    record = {
        "source_bytes": len(source_bytes),
        "source_sha256": hashlib.sha256(source_bytes).hexdigest(),
        "terminal_rows": 28,
        "first_text_to_visible_ms": first_visible,
        "typing_to_echo_in_burst_ms": echo_ms,
        "late_chunk_to_visible_frame_ms": {width: stats(values) for width, values in timings.items()},
        "reading_anchor": anchor,
        "identical_committed_text": True,
        "draft_preserved": True,
    }
    (client.directory / "stream-profile.json").write_text(
        json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(record, ensure_ascii=False))
