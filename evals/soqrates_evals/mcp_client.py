"""Drive `soqrates mcp` over stdio, as any MCP client would.

Each run gets a fresh, throwaway state directory and config directory, so
that runs never share a learner (every author starts as a novice, who gets
multiple-choice questions) and never read the user's own configuration.
"""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import dataclass
from typing import Any

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

#: The binary to run; build it with `cargo build --release -p soqrates-cli`.
BINARY_ENV = "SOQRATES_BIN"
DEFAULT_BINARY = "soqrates"

#: An author identity (ADR-0003 D1); any will do in a fresh state directory.
EVAL_AUTHOR = "github.com:1"


@dataclass
class Session:
    """One graded session: what `start_session` asked and what
    `submit_answers` explained."""

    questions: list[dict[str, Any]]
    feedback: list[dict[str, Any]]


def _result_json(result: Any) -> dict[str, Any]:
    if result.isError:
        text = " ".join(getattr(c, "text", "") for c in result.content)
        raise RuntimeError(f"soqrates tool error: {text}")
    if result.structuredContent is not None:
        return result.structuredContent
    return json.loads(result.content[0].text)


async def run_session(diff: str, pick: int = 0) -> Session:
    """Start a session on `diff`, answer every multiple-choice question with
    option `pick` (clamped to the options offered) and every open question
    with an empty answer, and return the questions and the feedback.

    A picked wrong option's feedback is its reason followed by the
    question's explanation (ADR-0004 D1), so `pick` decides which reason is
    seen besides the explanation.
    """
    binary = os.environ.get(BINARY_ENV, DEFAULT_BINARY)
    with tempfile.TemporaryDirectory(prefix="soqrates-eval-") as tmp:
        env = dict(os.environ)  # keeps the provider's API key
        env["XDG_CONFIG_HOME"] = os.path.join(tmp, "config")
        params = StdioServerParameters(
            command=binary,
            args=["--state-dir", os.path.join(tmp, "state"), "mcp"],
            env=env,
            cwd=tmp,
        )
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                started = _result_json(
                    await session.call_tool(
                        "start_session", {"author": EVAL_AUTHOR, "diff": diff}
                    )
                )
                questions = started["questions"]
                answers = []
                for q in questions:
                    options = q.get("options")
                    if options:
                        answers.append(
                            {"question": q["id"], "choice": min(pick, len(options) - 1)}
                        )
                    else:
                        answers.append({"question": q["id"], "text": ""})
                if not answers:
                    return Session(questions=[], feedback=[])
                submitted = _result_json(
                    await session.call_tool(
                        "submit_answers",
                        {"session_key": started["session_key"], "answers": answers},
                    )
                )
                return Session(questions=questions, feedback=submitted["feedback"])
