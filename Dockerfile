FROM rust:1.98-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder

WORKDIR /build
COPY . .
RUN cargo build --release --locked \
    && strip target/release/stuntdouble

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/stuntdouble /usr/local/bin/stuntdouble

USER nobody
EXPOSE 8080
ENTRYPOINT ["stuntdouble"]
CMD ["serve", "--config", "/etc/stuntdouble/stuntdouble.toml"]
