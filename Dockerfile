FROM --platform=$BUILDPLATFORM rust:1.98-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder

# linux/amd64 builds natively; linux/arm64 is cross-compiled from the same
# host-platform builder, which keeps a release build minutes long instead of
# paying for a fully emulated Boa compile.
ARG TARGETARCH

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) \
        ;; \
      arm64) \
        apt-get update; \
        apt-get install --yes --no-install-recommends \
          gcc-aarch64-linux-gnu \
          libc6-dev-arm64-cross; \
        rm -rf /var/lib/apt/lists/*; \
        ;; \
      *) \
        echo "unsupported TARGETARCH: $TARGETARCH" >&2; \
        exit 1; \
        ;; \
    esac

WORKDIR /build
COPY . .

# rust-toolchain.toml pins `stable`, which rustup installs as a second
# toolchain, so the cross target is added here rather than in the builder setup.
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) \
        target=x86_64-unknown-linux-gnu; \
        strip_cmd=strip; \
        ;; \
      arm64) \
        target=aarch64-unknown-linux-gnu; \
        strip_cmd=aarch64-linux-gnu-strip; \
        rustup target add "$target"; \
        ;; \
      *) \
        echo "unsupported TARGETARCH: $TARGETARCH" >&2; \
        exit 1; \
        ;; \
    esac; \
    cargo build --release --locked --target "$target"; \
    "$strip_cmd" "target/$target/release/stuntdouble"; \
    cp "target/$target/release/stuntdouble" /stuntdouble

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /stuntdouble /usr/local/bin/stuntdouble

USER nobody
EXPOSE 8080
ENTRYPOINT ["stuntdouble"]
CMD ["serve", "--config", "/etc/stuntdouble/stuntdouble.toml"]
