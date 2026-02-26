FROM debian:trixie-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
        default-mysql-client \
        gzip \
        awscli \
    && rm -rf /var/lib/apt/lists/*

COPY backup.sh .

USER nobody

ENTRYPOINT ["/app/backup.sh"]
