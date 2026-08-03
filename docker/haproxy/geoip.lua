-- geoip.lua — Country/ASN lookup for HAProxy community edition (HTTP fallback)
-- Loaded via: lua-load /usr/local/etc/haproxy/geoip.lua
-- Uses HTTP to geoip-lookup microservice at port 8001

local http = require("socket.http")
local ok_cjson, cjson = pcall(require, "cjson.safe")

-- Per-worker cache: src_ip -> {country, asn, expire}
local geo_cache = {}
local CACHE_TTL = 3600

core.register_action("geoip_lookup", {"http-req"}, function(txn)
    local src_ip = tostring(txn.f:src())
    txn:set_var("txn.geo_country", "")
    txn:set_var("txn.geo_asn", "")

    -- Serve from cache when fresh
    local cached = geo_cache[src_ip]
    if cached and cached.expire > os.time() then
        txn:set_var("txn.geo_country", cached.country)
        txn:set_var("txn.geo_asn", cached.asn)
        return
    end

    -- IPv6 addresses are unbracketed from txn.f:src(); bracket them so the
    -- downstream service can parse the path segment.
    local lookup_ip = src_ip
    if lookup_ip:find(":") then
        lookup_ip = "[" .. lookup_ip .. "]"
    end

    local geoip_url = "http://geoip-lookup:8001/geoip/" .. lookup_ip

    local response, status = http.request{
        url = geoip_url,
        timeout = 2,
        headers = { ["Connection"] = "close" }
    }

    local country, asn = "", ""
    if status == 200 and response then
        local decoded
        if ok_cjson then
            local decode_ok
            decode_ok, decoded = pcall(cjson.decode, response)
            if not decode_ok then
                decoded = nil
            end
        end

        if ok_cjson and type(decoded) == "table" then
            country = decoded.country_code or ""
            if decoded.asn ~= nil and decoded.asn ~= cjson.null then
                asn = tostring(decoded.asn)
            end
        else
            -- Fall back to pattern parsing if cjson is unavailable
            country = response:match('"country_code"%s*:%s*"([^"]*)"') or ""
            local asn_match = response:match('"asn"%s*:%s*"?([^",}]*)"?')
            if asn_match and asn_match ~= "null" then
                asn = asn_match
            end
        end
    else
        core.Warning("geoip lookup failed for " .. lookup_ip .. ": " .. tostring(status or "no response"))
    end

    txn:set_var("txn.geo_country", country)
    txn:set_var("txn.geo_asn", asn)

    -- Cache the result (including empty results) for CACHE_TTL seconds
    geo_cache[src_ip] = { country = country, asn = asn, expire = os.time() + CACHE_TTL }
end, 0)
