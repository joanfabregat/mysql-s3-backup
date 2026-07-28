#!/usr/bin/env bash
set -euo pipefail

# --- Configuration ---

MYSQL_PORT="${MYSQL_PORT:-3306}"
S3_PREFIX="${S3_PREFIX:-/}"
S3_STORAGE_CLASS="${S3_STORAGE_CLASS-STANDARD_IA}"

# Wall-clock ceilings. Without these a stalled dump or upload hangs forever,
# and on Kubernetes a CronJob with `concurrencyPolicy: Forbid` then skips every
# subsequent run while looking idle -- that is how one deployment of this image
# silently stopped backing up for 50 days. A Job-level activeDeadlineSeconds is
# the outer net; these are the inner one, and they give a precise error instead
# of an opaque kill.
#
# mysqldump can block indefinitely on a metadata lock; s5cmd can stall on a
# network read. Both are wrapped.
#
# THESE MUST FIT INSIDE THE JOB'S activeDeadlineSeconds, or the outer net always
# fires first and these never get to say which step stalled -- which defeats the
# point of having them. The worst case is:
#
#   per database = DUMP_TIMEOUT + (max_attempts x UPLOAD_TIMEOUT)
#   total        = that x number of databases in MYSQL_DATABASE
#
# At the defaults below with 3 upload attempts that is 600 + 900 = 1500s per
# database, so two databases fit in 3000s -- inside the 3600s
# activeDeadlineSeconds both current deployments use. The first version of this
# shipped 1800/900, which came to 4500s for one database and 9000s for two, so
# the Job deadline always won and these never fired.
#
# For scale: the largest current deployment dumps and uploads a ~924 MiB gzipped
# database in about 2m05s, so 600/300 is roughly 5x observed. Raise them for a
# genuinely larger database -- and raise activeDeadlineSeconds to match.
DUMP_TIMEOUT="${DUMP_TIMEOUT:-600}"
UPLOAD_TIMEOUT="${UPLOAD_TIMEOUT:-300}"
# Seconds to wait after TERM before sending KILL.
TIMEOUT_GRACE="${TIMEOUT_GRACE:-30}"

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

# --- Build s5cmd arguments ---

s5cmd_global_args=()
if [[ -n "${S3_ENDPOINT_URL:-}" ]]; then
    s5cmd_global_args+=(--endpoint-url "$S3_ENDPOINT_URL")
fi

s5cmd_cp_args=()
if [[ -n "$S3_STORAGE_CLASS" ]]; then
    s5cmd_cp_args+=(--storage-class "$S3_STORAGE_CLASS")
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

    # NOTE: the dump is buffered to /tmp rather than streamed to S3 (s5cmd has
    # a `pipe` subcommand that would allow streaming). That is deliberate: it
    # means only a COMPLETE dump is ever uploaded. Streaming would fuse dump and
    # upload, so a mysqldump that died halfway could leave a truncated object
    # that looks complete -- which matters a great deal when the target bucket
    # is under Object Lock and the bad object cannot be deleted.
    #
    # The cost is disk: /tmp is ephemeral storage on Kubernetes and its limit is
    # enforced by eviction. Size it above the largest single dump, not the sum,
    # since each is removed after upload.
    if ! timeout -k "$TIMEOUT_GRACE" "$DUMP_TIMEOUT" \
            mysqldump "${mysqldump_args[@]}" "$db" | gzip -c > "$dump_file"; then
        status=$?
        # 124 is GNU coreutils' "timed out". BusyBox timeout (what Alpine ships,
        # and what this image runs) instead exits 128+signal -- 143 for the TERM
        # it sends at the deadline, 137 if the -k KILL was needed. Verified on
        # busybox 1.37.0 / alpine 3.23: `timeout -k 5 2 sleep 60` returns 143.
        if (( status == 124 || status == 137 || status == 143 )); then
            echo "Error: dump of '$db' exceeded ${DUMP_TIMEOUT}s and was killed" >&2
        else
            echo "Error: dump of '$db' failed (exit ${status})" >&2
        fi
        rm -f "$dump_file"
        exit 1
    fi

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
    until timeout -k "$TIMEOUT_GRACE" "$UPLOAD_TIMEOUT" \
            s5cmd "${s5cmd_global_args[@]}" cp "${s5cmd_cp_args[@]}" "$dump_file" "$s3_uri"; do
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
