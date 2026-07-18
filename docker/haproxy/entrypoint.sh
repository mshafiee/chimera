#!/bin/sh

set -e

mkdir -p /var/log/haproxy

rsyslogd

exec haproxy -f /usr/local/etc/haproxy/haproxy.cfg -db "$@"