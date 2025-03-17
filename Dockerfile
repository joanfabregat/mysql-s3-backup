# --- Builder Stage ---
FROM python:3.13-slim-bookworm AS builder
WORKDIR /app

# Install uv and its dependencies
COPY --from=ghcr.io/astral-sh/uv:0.5.31 /uv /uvx /bin/
RUN chmod +x /bin/uv /bin/uvx && \
    uv venv .venv --python 3.13
ENV PATH="/app/.venv/bin:$PATH"

# Copy dependency specification and install production dependencies
COPY uv.lock pyproject.toml ./
RUN uv sync --frozen


# --- Final Image ---
FROM python:3.13-slim-bookworm AS final
WORKDIR /app

RUN apt-get update && apt-get install -y default-mysql-client gzip

ARG MYSQL_PORT=3306
ARG S3_PREFIX=/

ENV MYSQL_PORT=$MYSQL_PORT
ENV S3_PREFIX=$S3_PREFIX

COPY --from=builder /app/.venv .venv
ENV PATH="/app/.venv/bin:$PATH"

COPY run.py .

CMD ["python", "run.py"]