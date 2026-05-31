#!/usr/bin/env python3
"""Live HTTP verifier for the todos_api Cargo example.

This script intentionally uses only the Python standard library plus Cargo. It
starts `cargo run --example todos_api` as a child process with an isolated
SQLite file and localhost listener, then exercises the public HTTP contract over
real TCP.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

STARTUP_TIMEOUT_SECONDS = 90.0
HTTP_TIMEOUT_SECONDS = 3.0
SHUTDOWN_TIMEOUT_SECONDS = 5.0
POLL_INTERVAL_SECONDS = 0.1

BAD_REQUEST_PROBLEM = {
    "status": 400,
    "error": "bad_request",
    "message": "bad request",
}
VALIDATION_PROBLEM = {
    "status": 422,
    "error": "validation_failed",
    "message": "validation failed",
}
NOT_FOUND_PROBLEM = {
    "status": 404,
    "error": "not_found",
    "message": "resource not found",
}
INTERNAL_PROBLEM = {
    "status": 500,
    "error": "internal_server_error",
    "message": "internal server error",
}

URL_RE = re.compile(r"todos_api listening on (http://[^\s]+)")
BOUND_ADDR_RE = re.compile(r"bound_addr=([^\s]+)")


class VerificationFailure(AssertionError):
    """Raised when a verification phase fails with diagnosable context."""


@dataclass(frozen=True)
class HttpResponse:
    status: int
    headers: Mapping[str, str]
    body: bytes

    @property
    def text(self) -> str:
        return self.body.decode("utf-8", errors="replace")


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    temp_dir = Path(tempfile.mkdtemp(prefix="ruw_todos_api_"))
    log_path = temp_dir / "todos_api.log"
    db_path = temp_dir / "todos.sqlite"
    proc: subprocess.Popen[bytes] | None = None
    success = False

    try:
        proc = start_example(repo_root, db_path, log_path)
        base_url = wait_for_ready(proc, log_path)
        run_contract_checks(proc, base_url, db_path, log_path)
        success = True
        print("PASS: live todos_api CRUD verifier completed successfully")
        return 0
    except VerificationFailure as error:
        print(f"FAIL: {error}", file=sys.stderr)
        print(f"diagnostic log: {log_path}", file=sys.stderr)
        if log_path.exists():
            print("--- child log tail ---", file=sys.stderr)
            print(log_tail(log_path), file=sys.stderr)
            print("--- end child log tail ---", file=sys.stderr)
        print(f"temporary directory retained: {temp_dir}", file=sys.stderr)
        return 1
    finally:
        if proc is not None:
            terminate_process_tree(proc)
        if success:
            shutil.rmtree(temp_dir, ignore_errors=True)


def start_example(repo_root: Path, db_path: Path, log_path: Path) -> subprocess.Popen[bytes]:
    database_url = f"sqlite://{db_path}?mode=rwc"
    env = os.environ.copy()
    env.update(
        {
            "RUW_TODOS_ADDR": "127.0.0.1:0",
            "RUW_TODOS_DATABASE_URL": database_url,
        }
    )

    log_file = log_path.open("wb")
    try:
        proc = subprocess.Popen(
            ["cargo", "run", "--example", "todos_api"],
            cwd=repo_root,
            env=env,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except FileNotFoundError as error:
        log_file.close()
        fail("startup", "Cargo executable was not found on PATH", detail=str(error))
    except OSError as error:
        log_file.close()
        fail("startup", "failed to spawn cargo run", detail=str(error))

    # The child owns the duplicated file descriptor after spawn; closing our
    # handle lets later reads see data without waiting for script shutdown.
    log_file.close()
    print(f"started todos_api via cargo run; log={log_path}")
    return proc


def wait_for_ready(proc: subprocess.Popen[bytes], log_path: Path) -> str:
    deadline = time.monotonic() + STARTUP_TIMEOUT_SECONDS
    base_url: str | None = None
    last_health_error = "not attempted"

    while time.monotonic() < deadline:
        assert_child_running(proc, "startup")
        base_url = base_url or announced_base_url(log_path)
        if base_url is not None:
            try:
                response = http_request(proc, base_url, "GET", "/health", phase="readiness")
                assert_status("readiness /health", response, 200)
                body = parse_json("readiness /health", response)
                assert_equal("readiness /health body", body, {"status": "ok"})
                print(f"ready: {base_url}")
                return base_url
            except VerificationFailure as error:
                last_health_error = str(error)
        time.sleep(POLL_INTERVAL_SECONDS)

    if base_url is None:
        fail(
            "startup",
            f"timed out after {STARTUP_TIMEOUT_SECONDS:.0f}s waiting for bound address announcement",
            log_path=log_path,
        )

    fail(
        "readiness",
        f"timed out after {STARTUP_TIMEOUT_SECONDS:.0f}s waiting for /health",
        detail=last_health_error,
        log_path=log_path,
    )


def announced_base_url(log_path: Path) -> str | None:
    if not log_path.exists():
        return None

    text = log_path.read_text(encoding="utf-8", errors="replace")
    match = URL_RE.search(text)
    if match:
        return match.group(1).rstrip("/")

    # Fallback for tracing-subscriber log lines, e.g.:
    # "todos_api example is listening bound_addr=127.0.0.1:54321".
    match = BOUND_ADDR_RE.search(text)
    if match:
        return f"http://{match.group(1).rstrip('/')}"

    return None


def run_contract_checks(
    proc: subprocess.Popen[bytes], base_url: str, db_path: Path, log_path: Path
) -> None:
    print("checking happy-path CRUD over live HTTP")
    created = post_json(proc, base_url, "/todos", {"title": "ship live verifier"}, "create todo")
    assert_status("create todo", created, 201)
    created_body = parse_json("create todo", created)
    assert_todo("create todo body", created_body, title="ship live verifier", completed=False)
    todo_id = created_body["id"]
    assert_header("create todo Location", created, "Location", f"/todos/{todo_id}")

    listed = get_json(proc, base_url, "/todos", "list todos")
    assert_equal("list todos body", listed, [created_body])

    fetched = get_json(proc, base_url, f"/todos/{todo_id}", "fetch todo")
    assert_equal("fetch todo body", fetched, created_body)

    patched = patch_json(
        proc,
        base_url,
        f"/todos/{todo_id}",
        {"title": "ship verified API", "completed": True},
        "update todo",
    )
    assert_status("update todo", patched, 200)
    updated_body = parse_json("update todo", patched)
    assert_todo("update todo body", updated_body, title="ship verified API", completed=True)
    assert_equal("update todo id", updated_body["id"], todo_id)

    deleted = http_request(proc, base_url, "DELETE", f"/todos/{todo_id}", phase="delete todo")
    assert_status("delete todo", deleted, 204)
    assert_equal("delete todo body", deleted.body, b"")

    print("checking public error paths over live HTTP")
    bad_id = http_request(proc, base_url, "GET", "/todos/not-a-number", phase="malformed id")
    assert_problem("malformed id", bad_id, 400, BAD_REQUEST_PROBLEM)

    malformed_json = http_request(
        proc,
        base_url,
        "POST",
        "/todos",
        phase="malformed json",
        raw_body=b"{ not valid json",
        headers={"Content-Type": "application/json"},
    )
    assert_problem("malformed json", malformed_json, 400, BAD_REQUEST_PROBLEM)

    missing = http_request(proc, base_url, "GET", f"/todos/{todo_id}", phase="missing todo")
    assert_problem("missing todo", missing, 404, NOT_FOUND_PROBLEM)

    blank_title = post_json(proc, base_url, "/todos", {"title": "   "}, "blank title")
    assert_problem("blank title", blank_title, 422, VALIDATION_PROBLEM)
    after_blank_rejection = get_json(proc, base_url, "/todos", "list after blank title")
    assert_equal("blank title should not mutate collection", after_blank_rejection, [])

    print("checking sanitized database failure over live HTTP")
    drop_todos_table(db_path)
    db_failure = http_request(proc, base_url, "GET", "/todos", phase="database failure")
    assert_problem("database failure", db_failure, 500, INTERNAL_PROBLEM)
    assert_no_diagnostics_leaked("database failure body", db_failure.text, db_path, log_path)


def http_request(
    proc: subprocess.Popen[bytes],
    base_url: str,
    method: str,
    path: str,
    *,
    phase: str,
    raw_body: bytes | None = None,
    headers: Mapping[str, str] | None = None,
) -> HttpResponse:
    assert_child_running(proc, phase)
    url = f"{base_url}{path}"
    request = urllib.request.Request(
        url,
        data=raw_body,
        headers=dict(headers or {}),
        method=method,
    )

    try:
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_SECONDS) as response:
            return HttpResponse(
                status=response.status,
                headers=dict(response.headers.items()),
                body=response.read(),
            )
    except urllib.error.HTTPError as error:
        return HttpResponse(
            status=error.code,
            headers=dict(error.headers.items()),
            body=error.read(),
        )
    except TimeoutError as error:
        fail(phase, f"HTTP timeout requesting {method} {path}", detail=str(error))
    except urllib.error.URLError as error:
        assert_child_running(proc, phase)
        fail(phase, f"HTTP error requesting {method} {path}", detail=str(error))


def get_json(proc: subprocess.Popen[bytes], base_url: str, path: str, phase: str) -> Any:
    response = http_request(proc, base_url, "GET", path, phase=phase)
    assert_status(phase, response, 200)
    return parse_json(phase, response)


def post_json(
    proc: subprocess.Popen[bytes], base_url: str, path: str, body: Mapping[str, Any], phase: str
) -> HttpResponse:
    return json_request(proc, base_url, "POST", path, body, phase)


def patch_json(
    proc: subprocess.Popen[bytes], base_url: str, path: str, body: Mapping[str, Any], phase: str
) -> HttpResponse:
    return json_request(proc, base_url, "PATCH", path, body, phase)


def json_request(
    proc: subprocess.Popen[bytes],
    base_url: str,
    method: str,
    path: str,
    body: Mapping[str, Any],
    phase: str,
) -> HttpResponse:
    payload = json.dumps(body).encode("utf-8")
    return http_request(
        proc,
        base_url,
        method,
        path,
        phase=phase,
        raw_body=payload,
        headers={"Content-Type": "application/json"},
    )


def parse_json(phase: str, response: HttpResponse) -> Any:
    if not response.body:
        fail(phase, "expected JSON body but response body was empty", actual=response)
    try:
        return json.loads(response.text)
    except json.JSONDecodeError as error:
        fail(phase, "response body was not valid JSON", detail=str(error), actual=response.text)


def assert_status(phase: str, response: HttpResponse, expected: int) -> None:
    if response.status != expected:
        fail(phase, "unexpected HTTP status", expected=expected, actual=response)


def assert_header(phase: str, response: HttpResponse, name: str, expected: str) -> None:
    actual = header_value(response, name)
    if actual != expected:
        fail(phase, "unexpected response header", expected={name: expected}, actual={name: actual})


def header_value(response: HttpResponse, name: str) -> str | None:
    expected = name.lower()
    for key, value in response.headers.items():
        if key.lower() == expected:
            return value
    return None


def assert_problem(phase: str, response: HttpResponse, status: int, expected_body: Mapping[str, Any]) -> None:
    assert_status(phase, response, status)
    body = parse_json(phase, response)
    assert_equal(f"{phase} problem body", body, dict(expected_body))


def assert_todo(phase: str, body: Any, *, title: str, completed: bool) -> None:
    if not isinstance(body, dict):
        fail(phase, "todo response was not a JSON object", actual=body)
    if not isinstance(body.get("id"), int) or body["id"] <= 0:
        fail(phase, "todo id was not a positive integer", actual=body)
    expected = {"id": body["id"], "title": title, "completed": completed}
    assert_equal(phase, body, expected)


def assert_equal(phase: str, actual: Any, expected: Any) -> None:
    if actual != expected:
        fail(phase, "unexpected value", expected=expected, actual=actual)


def assert_no_diagnostics_leaked(phase: str, body_text: str, db_path: Path, log_path: Path) -> None:
    forbidden_fragments = [
        "sql",
        "sqlite",
        "todos",
        "table",
        "database",
        "connection",
        "seaorm",
        "sea_orm",
        "db_err",
        "no such",
        "ruw_todos_database_url",
        str(db_path),
        str(log_path.parent),
    ]
    lower_body = body_text.lower()
    leaked = [fragment for fragment in forbidden_fragments if fragment.lower() in lower_body]
    if leaked:
        fail(phase, "public error body leaked diagnostic text", expected="sanitized body", actual={"body": body_text, "leaked": leaked})


def drop_todos_table(db_path: Path) -> None:
    try:
        with sqlite3.connect(db_path) as connection:
            connection.execute("DROP TABLE todos")
            connection.commit()
    except sqlite3.Error as error:
        fail("database setup", "failed to drop todos table through sqlite3", detail=str(error), actual=str(db_path))


def assert_child_running(proc: subprocess.Popen[bytes], phase: str) -> None:
    exit_code = proc.poll()
    if exit_code is not None:
        fail(phase, "todos_api child process exited unexpectedly", actual={"exit_code": exit_code})


def terminate_process_tree(proc: subprocess.Popen[bytes]) -> None:
    if proc.poll() is not None:
        return

    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except OSError:
        proc.terminate()

    try:
        proc.wait(timeout=SHUTDOWN_TIMEOUT_SECONDS)
        return
    except subprocess.TimeoutExpired:
        pass

    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        return
    except OSError:
        proc.kill()
    proc.wait(timeout=SHUTDOWN_TIMEOUT_SECONDS)


def log_tail(log_path: Path, max_lines: int = 120) -> str:
    if not log_path.exists():
        return "<log file does not exist>"
    lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    return "\n".join(lines[-max_lines:]) or "<log file is empty>"


def fail(
    phase: str,
    message: str,
    *,
    expected: Any | None = None,
    actual: Any | None = None,
    detail: str | None = None,
    log_path: Path | None = None,
) -> None:
    parts = [f"phase={phase}", message]
    if expected is not None:
        parts.append(f"expected={format_value(expected)}")
    if actual is not None:
        parts.append(f"actual={format_value(actual)}")
    if detail:
        parts.append(f"detail={detail}")
    if log_path is not None:
        parts.append(f"log={log_path}")
    raise VerificationFailure("; ".join(parts))


def format_value(value: Any) -> str:
    if isinstance(value, HttpResponse):
        return json.dumps(
            {
                "status": value.status,
                "headers": dict(value.headers),
                "body": value.text,
            },
            sort_keys=True,
        )
    return repr(value)


if __name__ == "__main__":
    raise SystemExit(main())
