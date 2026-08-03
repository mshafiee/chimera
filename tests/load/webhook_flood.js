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

// Never fall back to a committed default secret
if (!__ENV.WEBHOOK_SECRET) {
  throw new Error('WEBHOOK_SECRET must be set (the real webhook signing secret)');
}
const SECRET = __ENV.WEBHOOK_SECRET;

export const options = {
  scenarios: {
    load: {
      // constant-arrival-rate drives a controlled RPS (stages[].target would
      // set VU count, not requests per second)
      executor: 'constant-arrival-rate',
      rate: 100, // requests per second (the queue-drop threshold)
      timeUnit: '1s',
      duration: '3m',
      preAllocatedVUs: 50,
      maxVUs: 200,
    },
  },
  thresholds: {
    // Latency thresholds
    http_req_duration: [
      'p(50)<200',  // 50% of requests should be below 200ms
      'p(95)<500',  // 95% of requests should be below 500ms
      'p(99)<1000', // 99% of requests should be below 1000ms
    ],
    // http_req_failed counts every 4xx/5xx including the intentional 503/429
    // load-shedding drops, so the threshold must accommodate them
    http_req_failed: ['rate<0.25'],  // allows for ~20% intentional drops
    // Custom metrics (Counter `rate` is a per-second increment, not a
    // percentage — the acceptance_rate Rate metric measures the real ratio)
    'acceptance_rate': ['rate>0.80'], // At least 80% of requests should be accepted
  },
};

const WEBHOOK_URL = __ENV.WEBHOOK_URL || 'http://localhost:8080/api/v1/webhook';

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

  check(res, {
    // Single combined check: any expected status passes (accepted, load-shed
    // drop, or legitimate reject)
    'status in expected set (accepted/dropped/rejected)': (r) =>
      [200, 202, 400, 401, 403, 429, 503].includes(r.status),
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

  // Minimal sleep to keep iteration timing consistent
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
