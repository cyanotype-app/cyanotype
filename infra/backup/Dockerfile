# Nightly Postgres backup to S3: pg_dump + rclone, driven by a tiny shell script (~90 MB image).
#   docker build -t cyanotype-backup infra/backup
FROM alpine:3.24
# postgresql17-client must match the server's major version (pgvector/pgvector:pg17).
RUN apk add --no-cache postgresql17-client rclone tzdata \
 && adduser -S -u 10001 backup
COPY backup.sh /usr/local/bin/backup.sh
USER backup
ENTRYPOINT ["/usr/local/bin/backup.sh"]
CMD ["daemon"]
