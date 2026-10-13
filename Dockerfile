# The rust:1 tag is deliberate: every rebuild picks up the latest stable
# toolchain, so a fix only needs a new image, not a Dockerfile edit.
FROM rust:1-alpine3.23 AS builder

# cmake and perl are aws-lc-sys's fallback build path, used when its bundled
# build script does not cover the target.
RUN apk add --no-cache build-base cmake perl

# cargo-auditable embeds the dependency list in the binary, so Trivy can
# report advisories in the Rust crates inside the image, not only in Alpine.
RUN cargo install cargo-auditable --locked --version 0.7.7

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo auditable build --release --locked

FROM alpine:3.23

# mariadb-client provides mariadb-dump (mysqldump) and the mariadb client the
# MYSQL_SINGLE_TRANSACTION engine check uses.
RUN apk upgrade --no-cache \
    && apk add --no-cache mariadb-client

COPY --from=builder /src/target/release/mysql-s3-backup /usr/local/bin/mysql-s3-backup

USER nobody

ENTRYPOINT ["/usr/local/bin/mysql-s3-backup"]
