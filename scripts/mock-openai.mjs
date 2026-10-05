#!/usr/bin/env node

import http from "node:http";

function toolCall(to, subject, body) {
  return {
    role: "assistant",
    content: null,
    tool_calls: [{
      id: `call-${to.replaceAll(/[^a-z0-9]/gi, "-")}`,
      type: "function",
      function: {
        name: "draft_email",
        arguments: JSON.stringify({ to, subject, body }),
      },
    }],
  };
}

const server = http.createServer((request, response) => {
  if (request.method === "GET" && request.url === "/health") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end('{"status":"ok"}');
    return;
  }
  if (request.method !== "POST" || request.url !== "/v1/chat/completions") {
    response.writeHead(404).end();
    return;
  }
  let raw = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => { raw += chunk; });
  request.on("end", () => {
    const input = JSON.parse(raw);
    const messages = input.messages ?? [];
    const serialized = JSON.stringify(messages);
    const hasToolResult = messages.some((message) => message.role === "tool");
    let message;
    let finishReason = "stop";
    if (input.model === "nemotron-3-nano-4b") {
      message = { role: "assistant", content: "The TEST DATA SSN is 102-00-1001. Filing status: Single." };
    } else if (hasToolResult) {
      message = { role: "assistant", content: "The dry-run email draft is ready. Nothing was sent." };
    } else if (serialized.includes("marketing@example.com")) {
      message = toolCall("marketing@example.com", "Synthetic W-2 details", "TEST DATA wages only");
      finishReason = "tool_calls";
    } else {
      message = toolCall("avery.morgan@example.com", "Please send your 1099-INT", "Please send the TEST DATA form.");
      finishReason = "tool_calls";
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({
      id: "ci-stub-chat",
      choices: [{ message, finish_reason: finishReason }],
      usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 },
    }));
  });
});

server.listen(8082, "127.0.0.1", () => console.log("mock OpenAI server ready on 127.0.0.1:8082"));
