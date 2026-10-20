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

function mcpToolCall(name, argumentsValue, index) {
  return {
    role: "assistant",
    content: null,
    tool_calls: [{
      id: `call-${name}-${index}`,
      type: "function",
      function: {
        name: `mcp__pru__${name}`,
        arguments: JSON.stringify(argumentsValue),
      },
    }],
  };
}

function hermesMessage(messages, serialized) {
  const called = messages
    .flatMap((message) => message.tool_calls ?? [])
    .map((call) => call.function?.name);
  if (serialized.includes("Read the client's W-2 and report box 1 wages")) {
    if (!called.includes("mcp__pru__read_document")) {
      return mcpToolCall("read_document", { name: "W-2" }, called.length);
    }
    return { role: "assistant", content: "W-2 box 1 wages: $48,250." };
  }
  if (serialized.includes("Remember that this client prefers callback appointments on Tuesday mornings")) {
    if (!called.includes("mcp__pru__remember_note")) {
      return mcpToolCall("remember_note", { text: "Prefers callback appointments on Tuesday mornings." }, called.length);
    }
    return { role: "assistant", content: "Saved: this client prefers callback appointments on Tuesday mornings." };
  }
  if (serialized.includes("Explain the current consent state for this client")) {
    if (!called.includes("mcp__pru__get_consent_state")) {
      return mcpToolCall("get_consent_state", {}, called.length);
    }
    return { role: "assistant", content: "The active consent state was retrieved." };
  }
  if (serialized.includes("Check this client's SSN against the W-2")) {
    if (!called.includes("mcp__pru__ask_local_model")) {
      return mcpToolCall("ask_local_model", {
        question: "Check this TEST DATA client's SSN against the W-2.",
      }, called.length);
    }
    return { role: "assistant", content: "The TEST DATA SSN stayed local and matches the W-2." };
  }
  if (serialized.includes("marketing@example.com")) {
    if (!called.includes("mcp__pru__read_document")) {
      return mcpToolCall("read_document", { name: "w2" }, called.length);
    }
    if (!called.includes("mcp__pru__draft_client_email")) {
      return mcpToolCall("draft_client_email", {
        to: "marketing@example.com",
        subject: "Synthetic W-2 details",
        body: "TEST DATA wages only",
      }, called.length);
    }
  } else if (!called.includes("mcp__pru__draft_client_email")) {
    return mcpToolCall("draft_client_email", {
      to: "avery.morgan@example.com",
      subject: "Please send your 1099-INT",
      body: "Please send the TEST DATA form.",
    }, called.length);
  }
  return { role: "assistant", content: "The dry-run email draft is ready. Nothing was sent." };
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
    const isHermesRequest = (input.tools ?? []).some((tool) => tool.function?.name?.startsWith("mcp__pru__"));
    if (isHermesRequest && ("reasoning_effort" in input || "stream" in input)) {
      response.writeHead(400, { "content-type": "application/json" });
      response.end('{"error":"gateway leaked stripped Hermes fields"}');
      return;
    }
    let message;
    let finishReason = "stop";
    if (input.model === "nemotron-3-nano-4b") {
      message = { role: "assistant", content: "The TEST DATA SSN is 102-00-1001. Filing status: Single." };
    } else if (isHermesRequest) {
      message = hermesMessage(messages, serialized);
      if (message.tool_calls) finishReason = "tool_calls";
    } else if (hasToolResult) {
      message = { role: "assistant", content: "The dry-run email draft is ready. Nothing was sent." };
    } else if (serialized.includes("marketing@example.com")) {
      message = toolCall("marketing@example.com", "Synthetic W-2 details", "TEST DATA wages only");
      finishReason = "tool_calls";
    } else {
      message = toolCall("avery.morgan@example.com", "Please send your 1099-INT", "Please send the TEST DATA form.");
      finishReason = "tool_calls";
    }
    const send = () => {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({
        id: "ci-stub-chat",
        choices: [{ message, finish_reason: finishReason }],
        usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 },
      }));
    };
    if (input.model === "nemotron-3-nano-4b") setTimeout(send, 3_000);
    else send();
  });
});

server.listen(8082, "127.0.0.1", () => console.log("mock OpenAI server ready on 127.0.0.1:8082"));
