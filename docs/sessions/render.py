#!/usr/bin/env python3
"""Render a Claude Code session transcript (JSONL) to a readable Markdown log.

Keeps the human-facing text only: Oliver's prompts and Claude's replies, with a one-line
note of which tools each reply used. Tool outputs, thinking and system notifications are
dropped. Usage: render.py <session.jsonl> <out.md> [title]
"""
import json, re, sys, collections
from datetime import datetime, timezone

src, out = sys.argv[1], sys.argv[2]
title = sys.argv[3] if len(sys.argv) > 3 else "Session log"
SYS = re.compile(r"<system-reminder>.*?</system-reminder>", re.S)

def ts(s):
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00")).astimezone().strftime("%Y-%m-%d %H:%M")
    except Exception:
        return s or ""

turns = []  # (role, time, text, tools)
with open(src) as f:
    for line in f:
        try:
            d = json.loads(line)
        except Exception:
            continue
        t = d.get("type")
        if t not in ("user", "assistant"):
            continue
        m = d.get("message") or {}
        content = m.get("content")
        blocks = content if isinstance(content, list) else [{"type": "text", "text": content}]
        if t == "user":
            texts = []
            for b in blocks:
                if isinstance(b, dict) and b.get("type") == "text":
                    txt = SYS.sub("", b.get("text") or "").strip()
                    # skip skill payloads, tool-loading notices and background-task notices,
                    # which the harness injects as user-role text
                    if (not txt or "[SYSTEM NOTIFICATION" in txt or "<task-notification>" in txt
                            or txt.startswith("Base directory for this skill:")
                            or txt.startswith("<system-reminder>") or txt == "Tool loaded."
                            or txt.startswith("# ")):  # an H1 is an injected reference, never a typed prompt
                        continue
                    texts.append(txt)
            if texts:
                turns.append(("Oliver", ts(d.get("timestamp")), "\n\n".join(texts), []))
        else:
            texts, tools = [], []
            for b in blocks:
                if not isinstance(b, dict):
                    continue
                if b.get("type") == "text" and (b.get("text") or "").strip():
                    texts.append(b["text"].strip())
                elif b.get("type") == "tool_use":
                    tools.append(b.get("name", "?"))
            if texts or tools:
                turns.append(("Claude", ts(d.get("timestamp")), "\n\n".join(texts), tools))

# merge consecutive Claude turns that carry only tool calls into the next text turn
merged = []
pending_tools = []
for role, when, text, tools in turns:
    if role == "Claude" and not text:
        pending_tools += tools
        continue
    if role == "Claude":
        tools = pending_tools + tools
        pending_tools = []
    merged.append((role, when, text, tools))

with open(out, "w") as w:
    w.write(f"# {title}\n\n")
    w.write(f"Rendered from the raw transcript `{src.split('/')[-1]}` by `docs/sessions/render.py`. "
            "Only the conversation is kept; tool output, reasoning and background-task notifications are omitted. "
            "Times are local.\n\n---\n\n")
    for role, when, text, tools in merged:
        w.write(f"### {role} · {when}\n\n{text}\n\n")
        if tools:
            c = collections.Counter(tools)
            w.write("_tools: " + ", ".join(f"{k}×{v}" if v > 1 else k for k, v in c.items()) + "_\n\n")
print(f"{len(merged)} turns → {out}")
