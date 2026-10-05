#!/usr/bin/env node

const baseUrl = process.env.PRU_BENCHMARK_URL ?? "http://127.0.0.1:8081";
const runsRequested = Number.parseInt(process.env.PRU_BENCHMARK_RUNS ?? "3", 10);

async function postJson(path, payload) {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  if (!response.ok) throw new Error(`${path} failed with HTTP ${response.status}`);
  return response.json();
}

async function exactPromptTokens() {
  const seed = [
    "You are reviewing a synthetic tax client file. Explain how a private assistant ",
    "should separate local sensitive work from consented remote work, record each decision, ",
    "and avoid sending identifiers outside the practice. ",
  ].join("");
  const tokenized = await postJson("/tokenize", { content: seed.repeat(12), add_special: true });
  if (!Array.isArray(tokenized.tokens) || tokenized.tokens.length < 300) {
    throw new Error(`tokenizer returned only ${tokenized.tokens?.length ?? 0} tokens`);
  }
  return tokenized.tokens.slice(0, 300);
}

async function runOnce(prompt, run) {
  const started = performance.now();
  const response = await fetch(`${baseUrl}/completion`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      prompt,
      n_predict: 150,
      ignore_eos: true,
      temperature: 0,
      seed: 1234,
      cache_prompt: false,
      return_tokens: true,
      stream: true,
    }),
  });
  if (!response.ok || !response.body) throw new Error(`/completion failed with HTTP ${response.status}`);

  let firstTokenSeconds;
  let finalEvent;
  let buffered = "";
  const decoder = new TextDecoder();
  for await (const chunk of response.body) {
    buffered += decoder.decode(chunk, { stream: true });
    const lines = buffered.split("\n");
    buffered = lines.pop() ?? "";
    for (const rawLine of lines) {
      const line = rawLine.trim();
      if (!line.startsWith("data: ")) continue;
      const event = JSON.parse(line.slice(6));
      if (firstTokenSeconds === undefined && (event.content || event.tokens?.length)) {
        firstTokenSeconds = (performance.now() - started) / 1000;
      }
      if (event.stop) finalEvent = event;
    }
  }
  if (firstTokenSeconds === undefined || !finalEvent) {
    throw new Error("stream did not include a generated token and final timing event");
  }
  const { prompt_n: promptTokens, predicted_n: outputTokens, predicted_per_second: tokensPerSecond } = finalEvent.timings;
  if (promptTokens !== 300 || outputTokens !== 150 || finalEvent.stop_type !== "limit") {
    throw new Error(`unexpected result: prompt=${promptTokens}, output=${outputTokens}, stop=${finalEvent.stop_type}`);
  }
  return {
    run,
    prompt_tokens: promptTokens,
    output_tokens: outputTokens,
    first_token_seconds: Number(firstTokenSeconds.toFixed(4)),
    output_tokens_per_second: Number(tokensPerSecond.toFixed(4)),
    stop_type: finalEvent.stop_type,
  };
}

const prompt = await exactPromptTokens();
const runs = [];
for (let index = 0; index < runsRequested; index += 1) runs.push(await runOnce(prompt, index + 1));
const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
console.log(JSON.stringify({
  protocol: { prompt_tokens: 300, output_tokens: 150, runs: runsRequested },
  runs,
  median_first_token_seconds: median(runs.map((run) => run.first_token_seconds)),
  median_output_tokens_per_second: median(runs.map((run) => run.output_tokens_per_second)),
}, null, 2));
