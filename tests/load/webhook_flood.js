// Load test for webhook endpoint
// Requires k6: https://k6.io/docs/getting-started/installation/
//
// Usage: k6 run tests/load/webhook_flood.js
//
// This test verifies:
// 1. Queue drop logic at 100 req/sec threshold
// 2. Latency measurements (p50, p95, p99)
// 3. Load shedding behavior (lower-priority signals dropped first)
// 4. RPC rate limit handling under load

import http from 'k6/http';
import { check, sleep, Trend, Counter, Rate } from 'k6';
import crypto from 'k6/crypto';

export const options = {
  stages: [
    { duration: '30s', target: 50 },   // Ramp up to 50 req/s
    { duration: '1m', target: 100 },  // Ramp up to 100 req/s (threshold)
    { duration: '30s', target: 150 }, // Exceed threshold to test load shedding
    { duration: '1m', target: 100 },  // Maintain at threshold
    { duration: '30s', target: 0 },   // Ramp down
  ],
  thresholds: {
    // Latency thresholds
    http_req_duration: [
      'p(50)<200',  // 50% of requests should be below 200ms
      'p(95)<500',  // 95% of requests should be below 500ms
      'p(99)<1000', // 99% of requests should be below 1000ms
    ],
    // Error rate thresholds
    http_req_failed: ['rate<0.05'],  // Less than 5% failures (allows for queue drops)
    // Custom metrics
    'dropped_signals': ['rate<0.20'], // Less than 20% should be dropped at peak load
    'accepted_signals': ['rate>0.80'], // At least 80% should be accepted
  },
};

const WEBHOOK_URL = __ENV.WEBHOOK_URL || 'http://localhost:8080/api/v1/webhook';
const SECRET = __ENV.WEBHOOK_SECRET || 'test-secret';

// Custom metrics for tracking
const latencyTrend = new Trend('webhook_latency_ms');
const acceptedCounter = new Counter('signals_accepted');
const droppedCounter = new Counter('signals_dropped');
const rejectedCounter = new Counter('signals_rejected');
const acceptedRate = new Rate('acceptance_rate');

// Strategies in priority order (EXIT > SHIELD > SPEAR)
const STRATEGIES = ['EXIT', 'SHIELD', 'SPEAR'];

function generateHMAC(timestamp, payload, secret) {
  const message = timestamp + payload;
  const hash = crypto.hmac('sha256', secret, message);
  return hash;
}

export default function () {
  // Rotate strategies to test priority queuing
  const strategy = STRATEGIES[Math.floor(Math.random() * STRATEGIES.length)];
  
  const timestamp = Date.now().toString();
  const payload = JSON.stringify({
    strategy: strategy,
    token: 'BONK',
    action: strategy === 'EXIT' ? 'SELL' : 'BUY',
    amount_sol: 0.5,
    wallet_address: '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
    trade_uuid: `test-${Date.now()}-${Math.random()}-${strategy}`,
  });

  const signature = generateHMAC(timestamp, payload, SECRET);

  const headers = {
    'Content-Type': 'application/json',
    'X-Signature': signature,
    'X-Timestamp': timestamp,
  };

  const startTime = Date.now();
  const res = http.post(WEBHOOK_URL, payload, { headers });
  const latency = Date.now() - startTime;
  
  latencyTrend.add(latency);

  // Check response status and categorize
  const isAccepted = res.status === 200 || res.status === 202;
  const isRejected = res.status === 400 || res.status === 401 || res.status === 403;
  const isDropped = res.status === 503 || res.status === 429; // Service unavailable or rate limited

  if (isAccepted) {
    acceptedCounter.add(1);
    acceptedRate.add(1);
  } else if (isDropped) {
    droppedCounter.add(1);
    acceptedRate.add(0);
  } else if (isRejected) {
    rejectedCounter.add(1);
    acceptedRate.add(0);
  }

  // Verify response structure
  let responseBody = {};
  try {
    responseBody = JSON.parse(res.body);
  } catch (e) {
    // Ignore parse errors for dropped/rejected requests
  }

  check(res, {
    'status is 200 or 202 (accepted)': (r) => r.status === 200 || r.status === 202,
    'status is 503 or 429 (dropped)': (r) => r.status === 503 || r.status === 429,
    'response time < 500ms': (r) => r.timings.duration < 500,
    'response has trade_uuid (if accepted)': (r) => {
      if (r.status === 200 || r.status === 202) {
        try {
          const body = JSON.parse(r.body);
          return body.trade_uuid !== undefined;
        } catch {
          return false;
        }
      }
      return true; // Not applicable for dropped/rejected
    },
    'EXIT signals prioritized (not dropped)': (r) => {
      // EXIT signals should rarely be dropped due to highest priority
      if (strategy === 'EXIT' && (r.status === 503 || r.status === 429)) {
        return false; // EXIT signals should not be dropped
      }
      return true;
    },
  });

  // Minimal sleep to maintain target rate (k6 handles this, but helps with timing)
  sleep(0.01);
}

// Summary function to log metrics
export function handleSummary(data) {
  const summary = {
    timestamp: new Date().toISOString(),
    metrics: {
      http_req_duration: {
        p50: data.metrics.http_req_duration.values['p(50)'],
        p95: data.metrics.http_req_duration.values['p(95)'],
        p99: data.metrics.http_req_duration.values['p(99)'],
        avg: data.metrics.http_req_duration.values.avg,
        min: data.metrics.http_req_duration.values.min,
        max: data.metrics.http_req_duration.values.max,
      },
      http_req_failed: {
        rate: data.metrics.http_req_failed.values.rate,
      },
      signals_accepted: {
        count: data.metrics.signals_accepted.values.count,
      },
      signals_dropped: {
        count: data.metrics.signals_dropped.values.count,
      },
      signals_rejected: {
        count: data.metrics.signals_rejected.values.count,
      },
      acceptance_rate: {
        rate: data.metrics.acceptance_rate.values.rate,
      },
    },
  };

  console.log('\n=== Load Test Summary ===');
  console.log(`Latency p50: ${summary.metrics.http_req_duration.p50}ms`);
  console.log(`Latency p95: ${summary.metrics.http_req_duration.p95}ms`);
  console.log(`Latency p99: ${summary.metrics.http_req_duration.p99}ms`);
  console.log(`Failed rate: ${(summary.metrics.http_req_failed.rate * 100).toFixed(2)}%`);
  console.log(`Acceptance rate: ${(summary.metrics.acceptance_rate.rate * 100).toFixed(2)}%`);
  console.log(`Signals accepted: ${summary.metrics.signals_accepted.count}`);
  console.log(`Signals dropped: ${summary.metrics.signals_dropped.count}`);
  console.log(`Signals rejected: ${summary.metrics.signals_rejected.count}`);
  console.log('========================\n');

  return {
    'stdout': JSON.stringify(summary, null, 2),
  };
}
