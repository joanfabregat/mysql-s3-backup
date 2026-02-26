#!/usr/bin/env bash
set -euo pipefail

# --- Configuration ---

MYSQL_PORT="${MYSQL_PORT:-3306}"
S3_PREFIX="${S3_PREFIX:-/}"
S3_STORAGE_CLASS="${S3_STORAGE_CLASS:-STANDARD_IA}"

# --- Parse DATABASE_URL if set ---

if [[ -n "${DATABASE_URL:-}" ]]; then
    # Validate scheme
    if [[ "$DATABASE_URL" != mysql://* ]]; then
        echo "Error: DATABASE_URL must use mysql:// scheme" >&2
        exit 1
    fi

    # Strip scheme
    url="${DATABASE_URL#mysql://}"

    # Extract user:password@host:port/path?query
    userinfo="${url%%@*}"
    rest="${url#*@}"

    MYSQL_USER="${userinfo%%:*}"
    MYSQL_PASSWORD="${userinfo#*:}"
    # If no colon in userinfo, password equals user — means no password
    if [[ "$MYSQL_PASSWORD" == "$MYSQL_USER" && "$userinfo" != *:* ]]; then
        MYSQL_PASSWORD=""
    fi

    # Split host:port/path?query
    hostport_path="${rest%%\?*}"
    query="${rest#*\?}"
    # If no query string, query equals hostport_path
    if [[ "$query" == "$hostport_path" ]]; then
        query=""
    fi

    hostport="${hostport_path%%/*}"
    path="${hostport_path#*/}"

    MYSQL_HOST="${hostport%%:*}"
    if [[ "$hostport" == *:* ]]; then
        MYSQL_PORT="${hostport#*:}"
    fi

    MYSQL_DATABASE="$path"

    # Parse unix_socket from query string
    if [[ -n "$query" && "$query" == *unix_socket=* ]]; then
        MYSQL_SOCKET="${query#*unix_socket=}"
        MYSQL_SOCKET="${MYSQL_SOCKET%%&*}"
    fi
fi

# --- Validate required variables ---

if [[ -z "${MYSQL_DATABASE:-}" ]]; then
    echo "Error: Missing required environment variable MYSQL_DATABASE" >&2
    exit 1
fi

if [[ -z "${MYSQL_USER:-}" ]]; then
    echo "Error: Missing required environment variable MYSQL_USER" >&2
    exit 1
fi

if [[ -z "${MYSQL_HOST:-}" && -z "${MYSQL_SOCKET:-}" ]]; then
    echo "Error: Missing required environment variable MYSQL_HOST or MYSQL_SOCKET" >&2
    exit 1
fi

if [[ -z "${S3_BUCKET:-}" ]]; then
    echo "Error: Missing required environment variable S3_BUCKET" >&2
    exit 1
fi

# --- Build base mysqldump arguments ---

mysqldump_args=(-u "$MYSQL_USER" --no-tablespaces)

if [[ -n "${MYSQL_SOCKET:-}" ]]; then
    mysqldump_args+=(--socket "$MYSQL_SOCKET")
else
    mysqldump_args+=(-h "$MYSQL_HOST" -P "$MYSQL_PORT")
fi

if [[ -n "${MYSQL_PASSWORD:-}" ]]; then
    mysqldump_args+=("--password=${MYSQL_PASSWORD}")
fi

# --- Build base aws s3 cp arguments ---

aws_args=()
if [[ -n "${S3_ENDPOINT_URL:-}" ]]; then
    aws_args+=(--endpoint-url "$S3_ENDPOINT_URL")
fi
if [[ -n "$S3_STORAGE_CLASS" ]]; then
    aws_args+=(--storage-class "$S3_STORAGE_CLASS")
fi

# --- Timestamp ---

timestamp=$(date -u +"%Y-%m-%dT%H%M%SZ")

# --- Process databases ---

IFS=',' read -ra databases <<< "$MYSQL_DATABASE"
multi=$(( ${#databases[@]} > 1 ))

for db in "${databases[@]}"; do
    db=$(echo "$db" | xargs) # trim whitespace
    [[ -z "$db" ]] && continue

    dump_file="/tmp/${db}_${timestamp}.sql.gz"
    echo "Dumping MySQL database: $db"

    mysqldump "${mysqldump_args[@]}" "$db" | gzip -c > "$dump_file"

    # Build S3 key
    prefix="${S3_PREFIX#/}"
    if (( multi )); then
        s3_key="${prefix}/${db}/${timestamp}.sql.gz"
    else
        s3_key="${prefix}/${timestamp}.sql.gz"
    fi
    s3_key="${s3_key#/}" # strip leading slash

    s3_uri="s3://${S3_BUCKET}/${s3_key}"
    echo "Uploading dump to S3: $s3_uri"

    # Retry upload up to 3 times
    attempt=0
    max_attempts=3
    until aws s3 cp "$dump_file" "$s3_uri" "${aws_args[@]}"; do
        attempt=$((attempt + 1))
        if (( attempt >= max_attempts )); then
            echo "Error: S3 upload failed after $max_attempts attempts" >&2
            rm -f "$dump_file"
            exit 1
        fi
        echo "Upload attempt $attempt failed, retrying in 5s..." >&2
        sleep 5
    done

    echo "Uploaded successfully."
    rm -f "$dump_file"
done

echo "Backup complete."
