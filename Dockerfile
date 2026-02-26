FROM debian:trixie-slim AS s5cmd-builder

ARG S5CMD_VERSION=2.3.0
ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && ARCH=$([ "$TARGETARCH" = "arm64" ] && echo "arm64" || echo "64bit") \
    && curl -sL "https://github.com/peak/s5cmd/releases/download/v${S5CMD_VERSION}/s5cmd_${S5CMD_VERSION}_Linux-${ARCH}.tar.gz" \
       | tar -xz -C /usr/local/bin s5cmd

FROM alpine:3.21
WORKDIR /app

RUN apk add --no-cache \
    bash \
    mariadb-client \
    gzip

COPY --from=s5cmd-builder /usr/local/bin/s5cmd /usr/local/bin/s5cmd

COPY backup.sh .

USER nobody

ENTRYPOINT ["/app/backup.sh"]
