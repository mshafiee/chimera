FROM haproxy:2.8-alpine

USER root

RUN apk add --no-cache \
        lua5.3 \
        lua5.3-socket

USER haproxy

RUN rm -f /usr/local/etc/haproxy/geoip.lua