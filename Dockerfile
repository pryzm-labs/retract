ARG NODE_IMAGE="node:24-bookworm-slim@sha256:3638d9a6fe4030bd716be989438248074489337ba3275657f93595428be4fc03"
ARG RUST_IMAGE="rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97"

FROM ${NODE_IMAGE} AS node-toolchain

FROM ${RUST_IMAGE} AS toolchain

ENV DEBIAN_FRONTEND=noninteractive \
    NPM_CONFIG_AUDIT=false \
    NPM_CONFIG_FUND=false \
    NPM_CONFIG_IGNORE_SCRIPTS=true \
    CARGO_NET_GIT_FETCH_WITH_CLI=false

# Tauri's Linux compile-time dependencies. Package installation is isolated in
# this toolchain layer; project builds below run as an unprivileged user.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libssl-dev \
        libwebkit2gtk-4.1-dev \
        libxdo-dev \
    && rm -rf /var/lib/apt/lists/*

# The official image omits optional rustup components. Install the exact
# project toolchain's checks before source is copied so source edits do not
# download and snapshot Clippy in a fresh dependency layer each time.
RUN rustup component add --toolchain 1.97.1 clippy rustfmt

COPY --from=node-toolchain /usr/local/bin/node /usr/local/bin/node
COPY --from=node-toolchain /usr/local/lib/node_modules/ /usr/local/lib/node_modules/
RUN ln -s /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s /usr/local/lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
    && useradd --create-home --uid 10001 --shell /bin/bash retract

ENV CARGO_HOME=/home/retract/.cargo \
    HOME=/home/retract \
    PATH=/home/retract/.cargo/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

WORKDIR /workspace
RUN chown retract:retract /workspace
USER retract

FROM toolchain AS dependencies

# Install exactly the integrity-checked npm lockfile without executing
# dependency lifecycle scripts. The local nanoid package is the only file:
# dependency and therefore must be present for npm ci.
COPY --chown=retract:retract package.json package-lock.json ./
COPY --chown=retract:retract vendor/nanoid/ ./vendor/nanoid/
RUN --mount=type=cache,id=retract-npm-cache,target=/home/retract/.npm,uid=10001,gid=10001,sharing=locked \
    npm ci --ignore-scripts --no-audit --no-fund

# Cargo fetch is the only Rust dependency step with network access. Both lock
# files are mandatory, and subsequent compilation is explicitly offline.
COPY --chown=retract:retract . .
RUN --mount=type=cache,id=retract-cargo-home,target=/home/retract/.cargo,uid=10001,gid=10001,sharing=locked \
    cargo fetch --locked --manifest-path crates/cleaner-domain/Cargo.toml \
    && cargo fetch --locked --manifest-path src-tauri/Cargo.toml

FROM dependencies AS checks

ARG TARGETARCH

ENV CARGO_INCREMENTAL=0 \
    CARGO_NET_OFFLINE=true \
    CARGO_PROFILE_DEV_DEBUG=0 \
    CARGO_PROFILE_TEST_DEBUG=0 \
    CARGO_TARGET_DIR=/home/retract/.cache/retract-target \
    CI=true

# Every project-controlled build/test command runs non-root and without a
# network. Dependency code may execute here, but it cannot reach credentials,
# the host filesystem, Docker's socket, or the network through this build.
RUN --network=none npm test
RUN --network=none npm run check:public-repo
RUN --network=none npm run build
RUN --network=none cargo fmt --manifest-path crates/cleaner-domain/Cargo.toml -- --check \
    && cargo fmt --manifest-path src-tauri/Cargo.toml -- --check

# Cargo's registry and compiled targets live in named BuildKit caches instead
# of immutable image layers. Source edits therefore reuse one bounded target
# tree rather than retaining another multi-gigabyte copy after every build.
RUN --network=none \
    --mount=type=cache,id=retract-cargo-home,target=/home/retract/.cargo,uid=10001,gid=10001,sharing=locked \
    --mount=type=cache,id=retract-cargo-target-${TARGETARCH},target=/home/retract/.cache/retract-target,uid=10001,gid=10001,sharing=locked \
    cargo test --offline --locked --manifest-path crates/cleaner-domain/Cargo.toml \
    && cargo clippy --offline --locked --manifest-path crates/cleaner-domain/Cargo.toml --all-targets -- -D warnings \
    && cargo test --offline --locked --manifest-path src-tauri/Cargo.toml \
    && cargo clippy --offline --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings

# Export this target with --output=type=local to obtain only the reviewed web
# assets. Native Tauri packages are intentionally produced on their target OS.
FROM scratch AS frontend-artifact
COPY --from=checks /workspace/dist/ /
