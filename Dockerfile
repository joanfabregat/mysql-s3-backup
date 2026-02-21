# --- Builder Stage ---
FROM python:3.14-slim-trixie AS builder
WORKDIR /app

# Install uv
COPY --from=ghcr.io/astral-sh/uv:0.10.4 /uv /uvx /bin/

# Copy dependency specification and install production dependencies
COPY uv.lock pyproject.toml ./
RUN uv sync --frozen --no-dev


# --- Final Image ---
FROM python:3.14-slim-trixie AS final
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends default-mysql-client gzip \
    && rm -rf /var/lib/apt/lists/*

ARG MYSQL_PORT=3306
ARG S3_PREFIX=/

ENV MYSQL_PORT=$MYSQL_PORT
ENV S3_PREFIX=$S3_PREFIX

COPY --from=builder /app/.venv .venv
ENV PATH="/app/.venv/bin:$PATH"
ENV PYTHONPATH="/app/src"

COPY src/ src/

CMD ["python", "-m", "mysql_s3_backup"]