#!/bin/sh

set -e

mkdir -p /var/log/haproxy 2>/dev/null || true

rsyslogd -n &
RSYSLOGD_PID=$!

# Wait for rsyslogd to come up (or fail) before starting HAProxy
for _ in 1 2 3 4 5; do
    [ -S /dev/log ] && break
    sleep 1
done
if ! kill -0 "$RSYSLOGD_PID" 2>/dev/null; then
    echo "ERROR: rsyslogd failed to start" >&2
    exit 1
fi

# Keep both processes under this shell so signals are forwarded and
# rsyslogd is not orphaned when the container stops.
trap 'kill "$RSYSLOGD_PID" 2>/dev/null || true' TERM INT

haproxy -f /usr/local/etc/haproxy/haproxy.cfg -db "$@" &
HAPROXY_PID=$!

wait "$HAPROXY_PID"
