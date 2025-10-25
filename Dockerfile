FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /target/release/api /usr/local/bin/

ENTRYPOINT ["api"]

EXPOSE 3000 
