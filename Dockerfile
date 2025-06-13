
FROM rust:slim-bullseye AS builder

WORKDIR /app

RUN apt-get update
# RUN apt-get -y install pkg-config librust-openssl-sys-dev --no-install-recommends

COPY . .

RUN cargo build --release

FROM debian:bullseye-slim

WORKDIR /app

COPY --from=builder /app/target/release/skope .


EXPOSE 9001

CMD ["./skope"]
