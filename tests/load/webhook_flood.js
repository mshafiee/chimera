// Load test for webhook endpoint
// Requires k6: https://k6.io/docs/getting-started/installation/
//
// Usage: k6 run tests/load/webhook_flood.js

import http from 'k6/http';
import { check, sleep } from 'k6';
import crypto from 'k6/crypto';

export const options = {
  stages: [
    { duration: '30s', target: 50 },   // Ramp up to 50 req/s
    { duration: '1m', target: 100 },  // Ramp up to 100 req/s
    { duration: '30s', target: 0 },   // Ramp down
  ],
  thresholds: {
    http_req_duration: ['p95<500'],  // 95% of requests should be below 500ms
    http_req_failed: ['rate<0.01'],  // Less than 1% failures
  },
};

const WEBHOOK_URL = __ENV.WEBHOOK_URL || 'http://localhost:8080/api/v1/webhook';
const SECRET = __ENV.WEBHOOK_SECRET || 'test-secret';

function generateHMAC(timestamp, payload, secret) {
  const message = timestamp + payload;
  const hash = crypto.hmac('sha256', secret, message);
  return hash;
}

export default function () {
  const timestamp = Date.now().toString();
  const payload = JSON.stringify({
    strategy: 'SHIELD',
    token: 'BONK',
    action: 'BUY',
    amount_sol: 0.5,
    wallet_address: '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
    trade_uuid: `test-${Date.now()}-${Math.random()}`,
  });

  const signature = generateHMAC(timestamp, payload, SECRET);

  const headers = {
    'Content-Type': 'application/json',
    'X-Signature': signature,
    'X-Timestamp': timestamp,
  };

  const res = http.post(WEBHOOK_URL, payload, { headers });

  check(res, {
    'status is 200 or 202': (r) => r.status === 200 || r.status === 202,
    'response time < 500ms': (r) => r.timings.duration < 500,
  });

  sleep(0.1); // Rate limit
}
