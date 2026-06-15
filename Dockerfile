# Build s5cmd from source with a current Go toolchain so the binary embeds an
# up-to-date Go stdlib. The upstream prebuilt release tarballs are compiled with
# an old Go (v2.3.0 ships Go 1.22.10), which Trivy flags for stdlib CVEs.
FROM golang:1.26-alpine AS s5cmd-builder

ARG S5CMD_VERSION=2.3.0
ENV CGO_ENABLED=0

RUN go install \
    -ldflags "-X=github.com/peak/s5cmd/v2/version.Version=v${S5CMD_VERSION}" \
    "github.com/peak/s5cmd/v2@v${S5CMD_VERSION}"

FROM alpine:3.23
WORKDIR /app

RUN apk upgrade --no-cache \
    && apk add --no-cache \
    bash \
    mariadb-client \
    gzip

COPY --from=s5cmd-builder /go/bin/s5cmd /usr/local/bin/s5cmd

COPY backup.sh .

USER nobody

ENTRYPOINT ["/app/backup.sh"]
