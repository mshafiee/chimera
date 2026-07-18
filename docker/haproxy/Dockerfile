FROM haproxy:2.8-alpine

USER root

RUN apk add --no-cache \
        lua5.3 \
        lua5.3-socket \
        rsyslog

COPY docker/haproxy/rsyslog.conf /etc/rsyslog.conf
COPY docker/haproxy/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 80 443

ENTRYPOINT ["/entrypoint.sh"]