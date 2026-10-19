#!/usr/bin/env node

import http from "node:http";

const gatewayUrl = process.env.PRU_GATEWAY_URL ?? "http://127.0.0.1:8787";
const agentSecret = process.env.PRU_AGENT_SECRET;
const hermesApiKey = process.env.PRU_HERMES_API_KEY;
const port = Number.parseInt(process.env.PRU_FAKE_HERMES_PORT ?? "8642", 10);

if (!agentSecret || !hermesApiKey) {
  throw new Error("PRU_AGENT_SECRET and PRU_HERMES_API_KEY are required");
}

const stats = {
  receivedSessions: [],
  completedSessions: [],
  deletedSessions: [],
  agentModelCalls: [],
  mcpCalls: [],
};

function sendJson(response, status, body) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(body));
}

async function readJson(request) {
  let raw = "";
  for await (const chunk of request) raw += chunk;
  return JSON.parse(raw);
}

function completion(turnId, content) {
  return {
    id: `fake-hermes-${turnId}`,
    choices: [{
      index: 0,
      message: { role: "assistant", content },
      finish_reason: "stop",
    }],
    usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  };
}

async function mcpRequest(turnId, method, params, id) {
  stats.mcpCalls.push({ turnId, method });
  const payload = { jsonrpc: "2.0", method, params };
  if (id !== undefined) payload.id = id;
  const response = await fetch(`${gatewayUrl}/mcp`, {
    method: "POST",
    headers: {
      accept: "application/json, text/event-stream",
      authorization: `Bearer ${agentSecret}`,
      "content-type": "application/json",
      "mcp-protocol-version": "2025-11-25",
    },
    body: JSON.stringify(payload),
  });
  if (!response.ok) {
    throw new Error(`MCP ${method} returned HTTP ${response.status}`);
  }
  if (response.status === 202) return null;
  return response.json();
}

async function modelRequest(turnId, messages, tools) {
  stats.agentModelCalls.push({ turnId });
  const response = await fetch(`${gatewayUrl}/agent/v1/chat/completions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${agentSecret}`,
      "content-type": "application/json",
      "x-pru-turn": turnId,
    },
    body: JSON.stringify({
      messages,
      tools,
      tool_choice: "auto",
      max_tokens: 600,
      reasoning_effort: "medium",
      stream: false,
    }),
  });
  if (!response.ok) return { denied: true, status: response.status };
  return { denied: false, body: await response.json() };
}

function openAiTools(tools) {
  return tools.map((tool) => ({
    type: "function",
    function: {
      name: `mcp__pru__${tool.name}`,
      description: tool.description,
      parameters: tool.inputSchema,
    },
  }));
}

async function runSession(turnId, input) {
  await mcpRequest(turnId, "initialize", {
    protocolVersion: "2025-11-25",
    capabilities: {},
    clientInfo: { name: "fake-hermes", version: "0.22.2" },
  }, 1);
  await mcpRequest(turnId, "notifications/initialized", {}, undefined);
  const listed = await mcpRequest(turnId, "tools/list", {}, 2);
  const tools = openAiTools(listed.result.tools);
  const messages = [...(input.messages ?? [])];

  for (let iteration = 0; iteration < 6; iteration += 1) {
    const model = await modelRequest(turnId, messages, tools);
    if (model.denied) {
      return completion(turnId, `Pru gateway refused model call with HTTP ${model.status}.`);
    }
    const message = model.body.choices?.[0]?.message;
    const calls = message?.tool_calls ?? [];
    if (calls.length === 0) return model.body;
    messages.push(message);

    for (const call of calls) {
      const prefixedName = call.function?.name ?? "";
      if (!prefixedName.startsWith("mcp__pru__")) {
        throw new Error(`unexpected tool name ${prefixedName}`);
      }
      const name = prefixedName.slice("mcp__pru__".length);
      const argumentsValue = JSON.parse(call.function.arguments ?? "{}");
      const tool = await mcpRequest(turnId, "tools/call", {
        name,
        arguments: argumentsValue,
      }, 3 + iteration);
      messages.push({
        role: "tool",
        tool_call_id: call.id,
        content: JSON.stringify(tool.result),
      });
      if (tool.result?.isError) {
        return completion(turnId, "Pru gateway refused the requested tool call.");
      }
    }
  }
  throw new Error("fake Hermes exceeded six model iterations");
}

const server = http.createServer(async (request, response) => {
  if (request.method === "GET" && request.url === "/health") {
    sendJson(response, 200, { status: "ok" });
    return;
  }
  if (request.method === "GET" && request.url === "/_test/stats") {
    sendJson(response, 200, stats);
    return;
  }
  if (request.headers.authorization !== `Bearer ${hermesApiKey}`) {
    sendJson(response, 401, { error: "unauthorized" });
    return;
  }
  if (request.method === "DELETE" && request.url?.startsWith("/api/sessions/")) {
    stats.deletedSessions.push(decodeURIComponent(request.url.slice("/api/sessions/".length)));
    sendJson(response, 200, { status: "deleted" });
    return;
  }
  if (request.method !== "POST" || request.url !== "/v1/chat/completions") {
    sendJson(response, 404, { error: "not found" });
    return;
  }
  const turnId = request.headers["x-hermes-session-id"];
  if (typeof turnId !== "string" || turnId.length === 0) {
    sendJson(response, 400, { error: "x-hermes-session-id is required" });
    return;
  }
  try {
    stats.receivedSessions.push(turnId);
    const result = await runSession(turnId, await readJson(request));
    stats.completedSessions.push(turnId);
    sendJson(response, 200, result);
  } catch (error) {
    sendJson(response, 500, { error: error instanceof Error ? error.message : String(error) });
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`fake Hermes ready on 127.0.0.1:${port}`);
});
