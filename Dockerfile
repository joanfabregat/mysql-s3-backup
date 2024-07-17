FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y default-mysql-client awscli gzip

ARG MYSQL_PORT=3306
ARG S3_PREFIX=/

ENV MYSQL_PORT=$MYSQL_PORT
ENV S3_PREFIX=$S3_PREFIX

WORKDIR /app
COPY run.sh .
RUN chmod +x run.sh

CMD ["sh", "run.sh"]