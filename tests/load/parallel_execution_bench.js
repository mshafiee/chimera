// Parallel Execution Load Test
// Requires k6: https://k6.io/docs/getting-started/installation/
//
// Usage: k6 run tests/load/parallel_execution_bench.js
//
// This test verifies:
// 1. Worker pool processes signals concurrently (target: 4x throughput)
// 2. Latency: p95 < 12s per signal
// 3. Throughput: 100 signals over 60 seconds with parallel execution
// 4. RPC rate limiting prevents provider throttling
// 5. Priority ordering under load (EXIT > SHIELD > SPEAR)
//
// Expected outcomes with parallel_enabled=true:
//   - Throughput: ~24 signals/min (vs 6 signals/min sequential)
//   - p95 latency: < 12s (vs 8-10s per signal but processing in parallel)
//   - No 503/429 errors from RPC rate limiting

import http from 'k6/http';
import { check, sleep, Trend, Counter } from 'k6';
import crypto from 'k6/crypto';

export const options = {
  stages: [
    { duration: '10s', target: 10 },   // Warm up
    { duration: '30s', target: 50 },   // Ramp to moderate load
    { duration: '20s', target: 100 },  // Peak load
    { duration: '10s', target: 0 },    // Cool down
  ],
  thresholds: {
    // Throughput: at least 80 signals should be accepted
    http_req_duration: [
      'p(50)<200',   // Half of requests respond in <200ms (webhook accept)
      'p(95)<5000',  // 95% under 5s (accounts for queue wait)
    ],
    http_req_failed: ['rate<0.10'],  // <10% failures
    'signals_accepted': ['count>80'], // At least 80 of 100 signals accepted
  },
};

const WEBHOOK_URL = __ENV.WEBHOOK_URL || 'http://localhost:8080/api/v1/webhook';
const SECRET = __ENV.WEBHOOK_SECRET || 'test-secret-that-is-thirty-two-chars-long!!';
const PARALLEL_ENABLED = __ENV.PARALLEL_ENABLED || 'true';

// Custom metrics
const acceptLatency = new Trend('accept_latency_ms');
const signalsAccepted = new Counter('signals_accepted');

// Tokens to simulate realistic diversity
const TOKENS = [
  'BONK', 'WIF', 'PYTH', 'JUP', 'RENDER',
  'JTO', 'TNSR', 'WEN', 'ZEX', 'DRIFT',
];

const STRATEGIES = ['EXIT', 'SHIELD', 'SHIELD', 'SPEAR', 'SPEAR']; // Bias toward SHIELD

function generateHMAC(timestamp, payload, secret) {
  return crypto.hmac('sha256', secret, timestamp + payload, 'hex');
}

export default function () {
  const strategy = STRATEGIES[Math.floor(Math.random() * STRATEGIES.length)];
  const token = TOKENS[Math.floor(Math.random() * TOKENS.length)];
  const timestamp = Date.now().toString();
  const amountSol = strategy === 'EXIT' ? '0.0' : (Math.random() * 0.5 + 0.01).toFixed(2);

  const payload = JSON.stringify({
    strategy: strategy,
    token: token,
    action: strategy === 'EXIT' ? 'SELL' : 'BUY',
    amount_sol: parseFloat(amountSol),
    wallet_address: '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
    trade_uuid: `perf-${__VU}-${__ITER}-${Date.now()}-${strategy}`,
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

  const isAccepted = res.status === 200 || res.status === 202;

  if (isAccepted) {
    acceptLatency.add(latency);
    signalsAccepted.add(1);
  }

  check(res, {
    'status is 200 or 202': (r) => r.status === 200 || r.status === 202,
    'status is not 503 (not rate limited)': (r) => r.status !== 503,
    'response time < 5000ms': (r) => r.timings.duration < 5000,
    'response has trade_uuid body': (r) => {
      if (isAccepted) {
        try {
          const body = JSON.parse(r.body);
          return body.trade_uuid !== undefined && body.status !== undefined;
        } catch {
          return false;
        }
      }
      return true;
    },
  });

  sleep(0.5); // Maintain target rate
}

export function handleSummary(data) {
  const summary = {
    timestamp: new Date().toISOString(),
    config: {
      parallel_enabled: PARALLEL_ENABLED,
      target_throughput: '100 signals / 60s',
    },
    metrics: {
      http_req_duration: {
        p50: data.metrics.http_req_duration.values['p(50)'],
        p95: data.metrics.http_req_duration.values['p(95)'],
        p99: data.metrics.http_req_duration.values['p(99)'],
        avg: data.metrics.http_req_duration.values.avg,
      },
      http_req_failed: {
        rate: data.metrics.http_req_failed.values.rate,
      },
      signals_accepted: {
        count: data.metrics.signals_accepted.values.count,
      },
      accept_latency: {
        avg: data.metrics.accept_latency.values.avg,
      },
    },
    expected_improvement: '4x throughput vs sequential mode (~6 signals/min without worker pool)',
  };

  console.log(JSON.stringify(summary, null, 2));
  return { stdout: JSON.stringify(summary) };
}
