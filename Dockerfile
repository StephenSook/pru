# syntax=docker/dockerfile:1.7

ARG LLAMA_CPP_COMMIT=a7fb71fab83b474a0892b9a05aaa3a8ddca2729b
ARG MODEL_REVISION=ba223d14e45525f7fae81db77ea8cabeb2fc6c25
ARG MODEL_FILE=NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf
ARG MODEL_SHA256=be5d9a656a51922f24f1f09a759cebb694e1f5d9728bf0ef9f8c972c5a0b5ef2

FROM nousresearch/hermes-agent:v0.22.2@sha256:36099926da316742423544e055d4734777105ffe6341873af5459b957d0c2919 AS hermes-slim
RUN rm -rf \
      /opt/hermes/tools/chromium-1208 \
      /opt/hermes/tools/ffmpeg-9.0.1-linux-x64 \
      /opt/hermes/uv-cache

FROM rust:1.89-bookworm AS rust-builder
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
RUN cargo build --locked --release -p pru-agent-jail -p pru-gateway

FROM node:22-bookworm-slim AS web-builder
WORKDIR /src/web
RUN corepack enable && corepack prepare pnpm@10.17.1 --activate
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web ./
RUN pnpm build

FROM debian:bookworm-slim AS llama-builder
ARG LLAMA_CPP_COMMIT
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates cmake git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN git clone https://github.com/ggml-org/llama.cpp.git \
    && cd llama.cpp \
    && git checkout "${LLAMA_CPP_COMMIT}" \
    && cmake -S . -B build \
      -DCMAKE_BUILD_TYPE=Release \
      -DGGML_NATIVE=OFF \
      -DLLAMA_BUILD_TESTS=OFF \
      -DLLAMA_BUILD_EXAMPLES=OFF \
      -DGGML_BACKEND_DL=ON \
      -DGGML_CPU_ALL_VARIANTS=ON \
    && cmake --build build --config Release --target llama-server -j2

FROM debian:bookworm-slim AS model
ARG MODEL_REVISION
ARG MODEL_FILE
ARG MODEL_SHA256
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /models \
    && curl --fail --location --retry 4 \
      "https://huggingface.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF/resolve/${MODEL_REVISION}/${MODEL_FILE}?download=true" \
      --output "/models/${MODEL_FILE}" \
    && echo "${MODEL_SHA256}  /models/${MODEL_FILE}" | sha256sum --check --strict

FROM node:22-bookworm-slim AS runtime-base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libgomp1 tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10000 hermes \
    && useradd --uid 10000 --gid 10000 --home-dir /tmp/hermes-home --no-create-home --shell /usr/sbin/nologin hermes
WORKDIR /app
ENV NODE_ENV=production \
    LD_LIBRARY_PATH=/usr/local/lib:/opt/llama \
    HOSTNAME=0.0.0.0 \
    PORT=3000 \
    PRU_BIND=127.0.0.1:8787 \
    PRU_GATEWAY_URL=http://127.0.0.1:8787 \
    PRU_LOCAL_LLM_URL=http://127.0.0.1:8081/v1 \
    PRU_LOCAL_LLM_HEALTH_URL=http://127.0.0.1:8081/health \
    PRU_LOCAL_LLM_MODEL=nemotron-3-nano-4b \
    PRU_DATA_DIR=/tmp/pru-data \
    PRU_AGENT=hermes \
    PRU_AGENT_REQUIRE_JAIL=1 \
    PRU_HERMES_URL=http://127.0.0.1:8642 \
    HERMES_HOME=/tmp/hermes-home \
    HERMES_RUNTIME_DIR=/tmp/hermes \
    RUST_LOG=pru_gateway=info
COPY --from=rust-builder /src/target/release/pru-gateway /usr/local/bin/pru-gateway
COPY --from=rust-builder /src/target/release/pru-agent-jail /usr/local/bin/pru-agent-jail
COPY --from=llama-builder /src/llama.cpp/build/bin/ /opt/llama/
COPY --from=hermes-slim /opt/hermes/ /opt/hermes/
COPY --from=hermes-slim /usr/local/lib/libsqlite3.so* /usr/local/lib/
COPY --from=web-builder /src/web/.next/standalone/ /app/web/
COPY --from=web-builder /src/web/.next/static/ /app/web/.next/static/
COPY scripts/benchmark-local-model.mjs /app/benchmark-local-model.mjs
COPY docker/hermes-config.yaml /etc/pru/hermes-config.yaml
COPY docker/entrypoint.sh /usr/local/bin/pru-entrypoint
RUN ln -s /opt/hermes/tools/python-3.14.7+20260901-linux-x64/bin/python3 /usr/local/bin/python3 \
    && chmod 0755 /usr/local/bin/pru-entrypoint \
    && HOME=/root/hermes-build-check HERMES_HOME=/root/hermes-build-check /opt/hermes/.venv/bin/hermes --version \
    && rm -rf /root/hermes-build-check \
    && pru-agent-jail --help >/dev/null
EXPOSE 3000
HEALTHCHECK --interval=10s --timeout=3s --start-period=210s --retries=6 CMD curl --fail --silent http://127.0.0.1:3000/healthz >/dev/null || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/pru-entrypoint"]

FROM runtime-base AS runtime
ARG MODEL_FILE
COPY --from=model "/models/${MODEL_FILE}" "/models/${MODEL_FILE}"
ENV PRU_MODEL_PATH=/models/NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf
CMD ["node", "/app/web/server.js"]
