-- geoip.lua — Country/ASN lookup for HAProxy community edition (HTTP fallback)
-- Loaded via: lua-load /usr/local/etc/haproxy/geoip.lua
-- Uses HTTP to geoip-lookup microservice at port 8001

local http = require("socket.http")

core.register_action("geoip_lookup", {"http-req"}, function(txn)
    local src_ip = tostring(txn.f:src())
    txn:set_var("txn.geo_country", "")
    txn:set_var("txn.geo_asn", "")

    local geoip_url = "http://geoip-lookup:8001/geoip/evaluate/" .. src_ip .. "?mode=default"

    local response, status, headers = http.request(geoip_url)

    if status == 200 and response then
        local country = response:match('"country_code"%s*:%s*"([^"]*)"')
        local asn = response:match('"asn"%s*:%s*"?([^",]*)"')

        if country then
            txn:set_var("txn.geo_country", country)
        end

        if asn and asn ~= "null" then
            txn:set_var("txn.geo_asn", asn)
        end
    end
end, 0)