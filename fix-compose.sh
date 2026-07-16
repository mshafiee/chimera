#!/bin/bash
# Fix docker-compose-haproxy.yml service references

sed -i '' 's/chimera-redis/redis/g' docker-compose-haproxy.yml
sed -i '' 's/chimera-prometheus/prometheus/g' docker-compose-haproxy.yml  
sed -i '' 's/chimera-grafana/grafana/g' docker-compose-haproxy.yml
sed -i '' 's/chimera-alertmanager/alertmanager/g' docker-compose-haproxy.yml

echo "Fixed service references:"
echo "  chimera-redis -> redis"
echo "  chimera-prometheus -> prometheus" 
echo "  chimera-grafana -> grafana"
echo "  chimera-alertmanager -> alertmanager"
