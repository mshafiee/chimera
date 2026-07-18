#!/bin/sh

set -e

mkdir -p /var/log/haproxy

rsyslogd -i /run/rsyslogd.pid

exec haproxy -f /usr/local/etc/haproxy/haproxy.cfg -db "$@"