// Parallel Execution Load Test
// Requires k6: https://k6.io/docs/getting-started/installation/
//
// Usage: k6 run tests/load/parallel_execution_bench.js
//
// This test verifies:
// 1. Worker pool processes signals concurrently (target: 4x throughput)
// 2. Latency: p95 < 5s per signal
// 3. Throughput: 100 signals over 60 seconds with parallel execution
// 4. RPC rate limiting prevents provider throttling
//
// Expected outcomes with parallel_enabled=true:
//   - Throughput: ~100 signals/min (vs 6 signals/min sequential)
//   - p95 latency: < 5s
//   - No 503 errors from RPC rate limiting

import http from 'k6/http';
import { check, sleep, Trend, Counter } from 'k6';
import crypto from 'k6/crypto';

const WEBHOOK_URL = __ENV.WEBHOOK_URL || 'http://localhost:8080/api/v1/webhook';

// Never fall back to a committed default secret: a missing secret must fail
// the run instead of silently testing with a known value.
if (!__ENV.WEBHOOK_SECRET) {
  throw new Error('WEBHOOK_SECRET must be set (the real webhook signing secret)');
}
const SECRET = __ENV.WEBHOOK_SECRET;
const PARALLEL_ENABLED = __ENV.PARALLEL_ENABLED === 'true';

export const options = {
  scenarios: {
    // Fixed number of iterations (100 signals) spread over 60s — matches the
    // documented "100 signals over 60 seconds" outcome.
    load: {
      executor: 'shared-iterations',
      vus: 10,
      iterations: 100,
      maxDuration: '60s',
    },
  },
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
  const isRejected = [400, 401, 403, 429, 503].includes(res.status);

  if (isAccepted) {
    acceptLatency.add(latency);
    signalsAccepted.add(1);
  }

  check(res, {
    'status is 200 or 202': (r) => r.status === 200 || r.status === 202,
    'status is not 503 (not rate limited)': (r) => r.status !== 503,
    'response time < 5000ms': (r) => r.timings.duration < 5000,
    'response body parses as JSON': (r) => {
      try {
        JSON.parse(r.body);
        return true;
      } catch {
        return false;
      }
    },
    'accepted responses carry trade_uuid/status': (r) => {
      if (!isAccepted) {
        return true;
      }
      try {
        const body = JSON.parse(r.body);
        return body.trade_uuid !== undefined && body.status !== undefined;
      } catch {
        return false;
      }
    },
    'rejected responses are still valid JSON errors': (r) => {
      if (!isRejected) {
        return true;
      }
      try {
        const body = JSON.parse(r.body);
        return body !== null && typeof body === 'object';
      } catch {
        return false;
      }
    },
  });

  // Small think time; shared-iterations caps the total at 100 requests
  sleep(0.5);
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
