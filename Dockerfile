FROM debian:trixie-slim AS s5cmd-builder

ARG S5CMD_VERSION=2.3.0
ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && ARCH=$([ "$TARGETARCH" = "arm64" ] && echo "arm64" || echo "64bit") \
    && curl -sL "https://github.com/peak/s5cmd/releases/download/v${S5CMD_VERSION}/s5cmd_${S5CMD_VERSION}_Linux-${ARCH}.tar.gz" \
       | tar -xz -C /usr/local/bin s5cmd

FROM debian:trixie-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
        default-mysql-client \
        gzip \
    && rm -rf /var/lib/apt/lists/*

COPY --from=s5cmd-builder /usr/local/bin/s5cmd /usr/local/bin/s5cmd

COPY backup.sh .

USER nobody

ENTRYPOINT ["/app/backup.sh"]
