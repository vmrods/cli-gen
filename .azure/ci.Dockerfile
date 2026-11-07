# The Linux CI checks, run as `docker build` on the SelfHosted-K8s pool.
#
# Why a build and not steps on the agent: the agents have no Rust, and their
# Docker daemon is a sidecar (DOCKER_HOST=tcp://localhost:2375), so bind
# mounts of the agent's checkout don't reach it. A build sends the source as
# its context instead, and the image pins the whole toolchain, so nothing
# depends on what an agent happens to have installed.
ARG RUST_VERSION
FROM rust:${RUST_VERSION}-bookworm AS tools
ARG CARGO_DENY_VERSION=0.20.2
ARG CARGO_LLVM_COV_VERSION=0.9.1
RUN rustup component add rustfmt clippy llvm-tools-preview \
 && cargo install --locked "cargo-deny@${CARGO_DENY_VERSION}" "cargo-llvm-cov@${CARGO_LLVM_COV_VERSION}"

FROM tools AS check
ENV CARGO_TERM_COLOR=always CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings"
WORKDIR /src
COPY . .
# A new value per run, so checks always run (new RustSec advisories appear
# without code changes); the cache mounts keep them incremental.
ARG RUN_ID
RUN cargo fmt --all --check
RUN --mount=type=cache,id=opencli-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=opencli-target,target=/src/target \
    cargo clippy --workspace --all-targets --locked -- -D warnings
RUN --mount=type=cache,id=opencli-registry,target=/usr/local/cargo/registry \
    cargo deny check
RUN --mount=type=cache,id=opencli-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=opencli-target,target=/src/target \
    cargo llvm-cov --workspace --locked --no-fail-fast --fail-under-lines 80
