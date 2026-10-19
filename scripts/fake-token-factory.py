#!/usr/bin/env python3
"""Deterministic OpenAI-compatible stub for the Hermes container smoke test."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(200, {"status": "ok"})
        else:
            self.send_json(404, {"error": "not found"})

    def do_POST(self) -> None:
        if self.path != "/v1/chat/completions":
            self.send_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("content-length", "0"))
        body = json.loads(self.rfile.read(length))
        messages = body.get("messages", [])
        has_tool_result = any(message.get("role") == "tool" for message in messages)
        if has_tool_result:
            message = {
                "role": "assistant",
                "content": "The TEST DATA client is Avery Morgan.",
            }
            finish_reason = "stop"
            completion_tokens = 11
        else:
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call_smoke_summary",
                        "type": "function",
                        "function": {
                            "name": "mcp__pru__get_client_summary",
                            "arguments": "{}",
                        },
                    }
                ],
            }
            finish_reason = "tool_calls"
            completion_tokens = 19
        self.send_json(
            200,
            {
                "id": "chatcmpl-pru-smoke",
                "object": "chat.completion",
                "created": 1_792_387_200,
                "model": "nvidia/Nemotron-3_5-Lightning",
                "choices": [
                    {
                        "index": 0,
                        "message": message,
                        "finish_reason": finish_reason,
                    }
                ],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": completion_tokens,
                    "total_tokens": 100 + completion_tokens,
                },
            },
        )

    def log_message(self, message: str, *args: object) -> None:
        print(f"token-factory-stub: {message % args}", flush=True)

    def send_json(self, status: int, body: object) -> None:
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8000), Handler).serve_forever()
