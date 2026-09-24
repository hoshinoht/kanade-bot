FROM rust:1.98-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY fixtures ./fixtures
RUN cargo build --locked --release

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251
# Twilight's platform TLS verifier reads the OS CA store, which the slim
# base does not ship; without this, outbound Discord TLS has no trust roots.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --create-home kanade
COPY --from=build /src/target/release/kanade-stack-spike /usr/local/bin/kanade-stack-spike
COPY --from=build /src/fixtures /app/fixtures
USER kanade
WORKDIR /app
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/kanade-stack-spike"]
CMD ["serve"]
