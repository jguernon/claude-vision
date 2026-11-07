#!/usr/bin/env python3
"""Builds a fake Claude Code home with demo sessions, agents and quotas, so screenshots
never show real sessions, prompts or file names.

Usage:
    python3 tools/demo/make_demo.py OUT_DIR      # prints the environment to use
    HOME=OUT_DIR/home XDG_CACHE_HOME=OUT_DIR/cache XDG_CONFIG_HOME=OUT_DIR/config \\
        claude-vision --lang en --snapshot docs/screenshot-main.png 124 34

Each demo session is tied to a short-lived `sleep` process so Claude Vision sees it as alive.
The processes end by themselves after a few minutes.
"""
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path

NOW = int(time.time() * 1000)


def iso(ms):
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(ms / 1000)) + f".{ms % 1000:03d}Z"


def spawn_alive():
    """Starts a detached sleep and returns (pid, procStart) like Claude Code records them."""
    p = subprocess.Popen(["sleep", "600"], start_new_session=True,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    stat = Path(f"/proc/{p.pid}/stat").read_text()
    proc_start = stat.rsplit(")", 1)[1].split()[19]
    return p.pid, proc_start


def slug(cwd):
    return "".join(c if c.isalnum() else "-" for c in cwd)


class Transcript:
    def __init__(self, sidechain=False, model="claude-opus-5-5", effort="high", advisor=None):
        self.lines = []
        self.sidechain = sidechain
        self.model = model
        self.effort = effort
        self.advisor = advisor
        self.ctx = 18_000

    def _base(self, typ, ts):
        d = {"type": typ, "timestamp": iso(ts), "isSidechain": self.sidechain}
        return d

    def prompt(self, ts, text):
        d = self._base("user", ts)
        d["message"] = {"role": "user", "content": text}
        self.lines.append(d)

    def assistant(self, ts, blocks, out_tokens=400, thinking_tokens=0):
        self.ctx += 3_000 + out_tokens
        d = self._base("assistant", ts)
        d["effort"] = self.effort
        if self.advisor:
            d["advisorModel"] = self.advisor
        d["message"] = {
            "role": "assistant",
            "model": self.model,
            "content": blocks,
            "usage": {
                "input_tokens": 4,
                "cache_read_input_tokens": self.ctx,
                "cache_creation_input_tokens": 1_200,
                "output_tokens": out_tokens,
                "output_tokens_details": {"thinking_tokens": thinking_tokens},
            },
        }
        self.lines.append(d)

    def think(self, ts, text, tokens):
        self.assistant(ts, [{"type": "thinking", "thinking": text}], tokens, tokens)

    def say(self, ts, text, tokens=300):
        self.assistant(ts, [{"type": "text", "text": text}], tokens)

    def tool(self, ts, name, inp, result, result_ts=None, tool_id=None):
        tid = tool_id or "toolu_" + uuid.uuid4().hex[:24]
        self.assistant(ts, [{"type": "tool_use", "id": tid, "name": name, "input": inp}], 120)
        if result is not None:
            d = self._base("user", result_ts or ts + 900)
            d["message"] = {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tid, "content": result, "is_error": False}]}
            self.lines.append(d)
        return tid

    def write(self, path):
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w") as f:
            for d in self.lines:
                f.write(json.dumps(d, separators=(",", ":"), ensure_ascii=False) + "\n")


def session(config, name, cwd, status, model, effort, started_min, builder, agents=()):
    pid, proc_start = spawn_alive()
    sid = str(uuid.uuid4())
    (config / "sessions").mkdir(parents=True, exist_ok=True)
    (config / "sessions" / f"{pid}.json").write_text(json.dumps({
        "pid": pid, "sessionId": sid, "cwd": cwd, "startedAt": NOW - started_min * 60_000,
        "procStart": proc_start, "version": "2.1.317", "kind": "interactive", "name": name,
        "status": status, "updatedAt": NOW - 20_000, "statusUpdatedAt": NOW - 20_000,
    }))
    tr = Transcript(model=model, effort=effort, advisor="claude-opus-5-5")
    tool_ids = builder(tr)
    base = config / "projects" / slug(cwd)
    tr.write(base / f"{sid}.jsonl")
    sub = base / sid / "subagents"
    for i, (agent_type, desc, working, steps) in enumerate(agents):
        aid = "a" + uuid.uuid4().hex[:16]
        sub.mkdir(parents=True, exist_ok=True)
        (sub / f"agent-{aid}.meta.json").write_text(json.dumps({
            "agentType": agent_type, "description": desc,
            "toolUseId": tool_ids[i] if i < len(tool_ids) else "", "spawnDepth": 1}))
        at = Transcript(sidechain=True, model="claude-haiku-5-5", effort="medium")
        at.prompt(NOW - 240_000, desc)
        t0 = NOW - (10_000 if working else 200_000) - len(steps) * 6_000
        for j, (tool, inp, res) in enumerate(steps):
            at.tool(t0 + j * 6_000, tool, inp, res, t0 + j * 6_000 + 1_500)
        at.write(sub / f"agent-{aid}.jsonl")
        mtime = (NOW - (4_000 if working else 200_000)) / 1000
        os.utime(sub / f"agent-{aid}.jsonl", (mtime, mtime))
        os.utime(sub / f"agent-{aid}.meta.json", (mtime, mtime))
    return sid, pid


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1]).resolve()
    home = out / "home"
    main_cfg = home / ".claude"
    work_cfg = home / ".claude-work"
    m = 60_000

    # ── session 1 : the busy one with agents (used for the map screenshot)
    def auth(tr):
        tr.prompt(NOW - 14 * m, "Refactor the auth middleware to use short-lived tokens, keep the API stable, and add tests.")
        tr.think(NOW - 14 * m + 4_000, "Plan the refactor.", 900)
        tr.tool(NOW - 13 * m, "Read", {"file_path": "/home/demo/acme-api/src/auth/middleware.ts"}, "export function authenticate(req, res, next) { ... }")
        tr.tool(NOW - 12 * m, "Grep", {"pattern": "verifyToken"}, "src/auth/middleware.ts\nsrc/routes/admin.ts\nsrc/routes/users.ts")
        ids = [
            tr.tool(NOW - 11 * m, "Agent", {"subagent_type": "Explore", "description": "Map token call sites", "prompt": "Find every place that verifies or issues a token."}, "Async agent launched successfully."),
            tr.tool(NOW - 10 * m, "Agent", {"subagent_type": "general-purpose", "description": "Write refresh-token tests", "prompt": "Add integration tests for token refresh."}, "Async agent launched successfully."),
            tr.tool(NOW - 9 * m, "Agent", {"subagent_type": "Plan", "description": "Migration plan for v2 tokens", "prompt": "Plan the rollout of short-lived tokens."}, None),
        ]
        tr.tool(NOW - 8 * m, "Edit", {"file_path": "/home/demo/acme-api/src/auth/tokens.ts"}, "The file was updated.")
        tr.tool(NOW - 7 * m, "Bash", {"description": "Run the auth test suite", "command": "npm test -- auth"}, "Tests: 42 passed, 42 total")
        d = tr._base("user", NOW - 6 * m)
        d["message"] = {"role": "user", "content": [{"type": "tool_result", "tool_use_id": ids[2], "content": "Plan ready: 3 phases, feature flag, dual-token window of 24 h.", "is_error": False}]}
        tr.lines.append(d)
        tr.tool(NOW - 5 * m, "Edit", {"file_path": "/home/demo/acme-api/src/auth/middleware.ts"}, "The file was updated.")
        tr.tool(NOW - 4 * m, "Bash", {"description": "Type-check the project", "command": "npx tsc --noEmit"}, "")
        tr.think(NOW - 3 * m, "Check the remaining call sites.", 400)
        tr.tool(NOW - 2 * m, "Read", {"file_path": "/home/demo/acme-api/src/routes/admin.ts"}, "router.get('/admin', authenticate, ...)")
        tr.tool(NOW - 60_000, "Edit", {"file_path": "/home/demo/acme-api/src/routes/admin.ts"}, "The file was updated.")
        tr.tool(NOW - 30_000, "Bash", {"description": "Run the full test suite", "command": "npm test"}, "Tests: 318 passed, 318 total")
        tr.say(NOW - 8_000, "Middleware now uses short-lived tokens; all 318 tests pass.", 520)
        return ids

    sid1, _ = session(main_cfg, "Refactor auth middleware", "/home/demo/acme-api", "busy",
                      "claude-opus-5-5", "high", 14, auth, agents=[
        ("Explore", "Map token call sites", True, [
            ("Grep", {"pattern": "issueToken"}, "src/auth/tokens.ts"),
            ("Read", {"file_path": "/home/demo/acme-api/src/auth/tokens.ts"}, "export function issueToken() {}"),
            ("Read", {"file_path": "/home/demo/acme-api/src/routes/users.ts"}, "router.post('/login', ...)"),
        ]),
        ("general-purpose", "Write refresh-token tests", True, [
            ("Write", {"file_path": "/home/demo/acme-api/test/refresh.test.ts"}, "File created."),
            ("Bash", {"description": "Run the new tests"}, "Tests: 6 passed"),
        ]),
        ("Plan", "Migration plan for v2 tokens", False, [
            ("Read", {"file_path": "/home/demo/acme-api/docs/auth.md"}, "# Auth"),
        ]),
    ])

    def e2e(tr):
        tr.prompt(NOW - 6 * m, "The checkout e2e test fails one run in five. Find out why and fix it.")
        ids = [tr.tool(NOW - 5 * m, "Agent", {"subagent_type": "general-purpose", "description": "Reproduce the flaky checkout test"}, "Async agent launched successfully.")]
        tr.tool(NOW - 3 * m, "Read", {"file_path": "/home/demo/storefront/e2e/checkout.spec.ts"}, "await page.click('#pay')")
        tr.tool(NOW - 40_000, "Bash", {"description": "Run the checkout test 20 times"}, "20/20 passed")
        return ids

    session(main_cfg, "Fix flaky e2e tests", "/home/demo/storefront", "busy", "claude-sonnet-5-5", "medium", 6, e2e,
            agents=[("general-purpose", "Reproduce the flaky checkout test", True, [
                ("Bash", {"description": "Loop the checkout test"}, "run 7: timeout on #pay"),
            ])])

    def notes(tr):
        tr.prompt(NOW - 50 * m, "Draft the release notes for 2.4 from the merged pull requests.")
        tr.tool(NOW - 48 * m, "Bash", {"description": "List merged pull requests"}, "#412 #415 #418 #421")
        tr.say(NOW - 45 * m, "Release notes drafted in CHANGELOG.md.")
        return []

    session(main_cfg, "Write release notes", "/home/demo/docs-site", "idle", "claude-sonnet-5-5", "low", 52, notes)

    def billing(tr):
        tr.prompt(NOW - 25 * m, "Migrate the billing service to the v2 payments API.")
        ids = [tr.tool(NOW - 20 * m, "Agent", {"subagent_type": "general-purpose", "description": "Port invoice endpoints"}, "Async agent launched successfully.")]
        tr.tool(NOW - 90_000, "Edit", {"file_path": "/home/demo/billing/src/payments/client.go"}, "The file was updated.")
        return ids

    session(work_cfg, "Migrate billing to v2", "/home/demo/billing", "busy", "claude-opus-5-5", "xhigh", 25, billing,
            agents=[("general-purpose", "Port invoice endpoints", True, [
                ("Edit", {"file_path": "/home/demo/billing/src/invoices/handler.go"}, "The file was updated."),
            ])])

    # ── quotas (cached values, so no API call is ever made)
    cache = out / "cache" / "claude-vision"
    cache.mkdir(parents=True, exist_ok=True)
    reset = lambda h: time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime(NOW / 1000 + h * 3600))
    (cache / "quotas.json").write_text(json.dumps({
        str(main_cfg.resolve()): {"at": NOW, "tier": "MAX 5x", "windows": [
            {"label": "5h", "pct": 37, "resets_at": reset(2.3)},
            {"label": "7d", "pct": 58, "resets_at": reset(73)},
            {"label": "7d Opus", "pct": 22, "resets_at": reset(73)}]},
        str(work_cfg.resolve()): {"at": NOW, "tier": "TEAM 5x", "windows": [
            {"label": "5h", "pct": 9, "resets_at": reset(4.1)},
            {"label": "7d", "pct": 84, "resets_at": reset(30)}]},
    }))
    for cfg in (main_cfg, work_cfg):
        (cfg / "sessions").mkdir(parents=True, exist_ok=True)
    (out / "config").mkdir(parents=True, exist_ok=True)

    print(f"HOME={home} XDG_CACHE_HOME={out / 'cache'} XDG_CONFIG_HOME={out / 'config'}")
    print(f"map session: {sid1[:8]}")


if __name__ == "__main__":
    main()
