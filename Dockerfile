# syntax=docker/dockerfile:1.7
#
# Barebone unbrowser image.
#
#   8.4 MB native binary on distroless/cc-debian12 (glibc + libgcc + libstdc++,
#   no shell, no package manager). Final image is ~13 MiB compressed pull size
#   (~48 MiB uncompressed on disk).
#
# Build (local, native arch):
#   docker build -t unbrowser:dev .
#
# One-off command:
#   docker run --rm unbrowser:dev navigate https://example.com --json
#
# MCP host:
#   docker run --rm -i unbrowser:dev --mcp
#
# Multi-arch publish happens in CI (.github/workflows/release.yml), where each
# arch's binary is already built natively — no QEMU, no in-container compile.
# This Dockerfile also self-builds from source for local/dev use.

############################################
# Stage 1 — build the native binary.
# glibc target (x86_64/aarch64-unknown-linux-gnu) => Debian, not Alpine.
# boring-sys2 (BoringSSL) needs cmake, ninja, and a C/C++ toolchain; bindgen
# (via wreq/boring-sys2) needs libclang. rust:1-bookworm already ships gcc/g++.
############################################
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        cmake ninja-build clang libclang-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Only what compile needs. profiles/*.toml and src/js/*.js are include_str!'d
# at compile time (see src/profile.rs), so they must be present here — but they
# do NOT need to be in the final image.
COPY Cargo.toml Cargo.lock ./
COPY src/       ./src/
COPY profiles/  ./profiles/
COPY prefit/    ./prefit/

# BuildKit cache mounts speed incremental + CI rebuilds (no-op on classic builder).
# Copy the binary out in the same RUN while target/ is still mounted.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --release --locked && \
    cp /build/target/release/unbrowser /unbrowser

############################################
# Stage 2 — runtime. distroless/cc-debian12:nonroot.
# glibc + libgcc + libstdc++ (BoringSSL is C++). Runs as UID 65532.
# No shell, no package manager — same family obscura ships (distroless/cc).
############################################
FROM gcr.io/distroless/cc-debian12:nonroot AS final

LABEL org.opencontainers.image.source="https://github.com/protostatis/unbrowser" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.title="unbrowser" \
      org.opencontainers.image.description="Agent-native browser. Single native binary. No Chrome."

COPY --from=builder /unbrowser /usr/local/bin/unbrowser

# `docker run img`             -> unbrowser --mcp   (default: MCP over stdio)
# `docker run img navigate …`  -> unbrowser navigate …   (args replace --mcp)
ENTRYPOINT ["unbrowser"]
CMD ["--mcp"]
