/**
 * Webhook Flood Load Test
 * 
 * Tests from PDD Phase 7:
 * - Send 100 webhooks/second
 * - Verify rate limiting kicks in
 * - Measure p99 latency under load (target: < 500ms)
 * 
 * Usage:
 *   k6 run webhook_flood.js
 *   k6 run --vus 100 --duration 30s webhook_flood.js
 * 
 * Install k6: https://k6.io/docs/getting-started/installation/
 */

import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate, Trend } from 'k6/metrics';
import crypto from 'k6/crypto';

// Custom metrics
const webhookAccepted = new Rate('webhook_accepted');
const webhookLatency = new Trend('webhook_latency');
const rateLimited = new Rate('rate_limited');

// Test configuration
export const options = {
    scenarios: {
        // Ramp up to 100 req/sec
        constant_load: {
            executor: 'constant-arrival-rate',
            rate: 100,           // 100 iterations per second
            timeUnit: '1s',
            duration: '30s',
            preAllocatedVUs: 50,
            maxVUs: 100,
        },
        // Burst test - spike to 200 req/sec
        burst_test: {
            executor: 'constant-arrival-rate',
            rate: 200,
            timeUnit: '1s',
            duration: '10s',
            preAllocatedVUs: 100,
            maxVUs: 200,
            startTime: '30s',   // Start after constant load
        },
    },
    thresholds: {
        // 95% of requests should complete within 500ms
        'webhook_latency': ['p(95)<500'],
        // At least 80% should be accepted (some rate limiting expected)
        'webhook_accepted': ['rate>0.8'],
        // Rate limited requests should be < 30% during normal load
        'rate_limited': ['rate<0.3'],
    },
};

// Configuration - update these for your environment
const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const WEBHOOK_SECRET = __ENV.WEBHOOK_SECRET || 'test-secret';

// Token list for variety
const TOKENS = ['BONK', 'WIF', 'PEPE', 'MYRO', 'POPCAT', 'MEW', 'PONKE', 'SLERF'];
const STRATEGIES = ['SHIELD', 'SPEAR'];

/**
 * Generate HMAC-SHA256 signature
 */
function generateSignature(timestamp, body) {
    const message = timestamp + body;
    const signature = crypto.hmac('sha256', WEBHOOK_SECRET, message, 'hex');
    return signature;
}

/**
 * Generate random signal payload
 */
function generatePayload() {
    return JSON.stringify({
        strategy: STRATEGIES[Math.floor(Math.random() * STRATEGIES.length)],
        token: TOKENS[Math.floor(Math.random() * TOKENS.length)],
        action: 'BUY',
        amount_sol: (Math.random() * 0.5 + 0.1).toFixed(4),
        wallet_address: '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
    });
}

/**
 * Main test function
 */
export default function () {
    const timestamp = Math.floor(Date.now() / 1000).toString();
    const body = generatePayload();
    const signature = generateSignature(timestamp, body);

    const params = {
        headers: {
            'Content-Type': 'application/json',
            'X-Signature': signature,
            'X-Timestamp': timestamp,
        },
        timeout: '10s',
    };

    const startTime = Date.now();
    const res = http.post(`${BASE_URL}/api/v1/webhook`, body, params);
    const latency = Date.now() - startTime;

    // Record latency
    webhookLatency.add(latency);

    // Check response
    const accepted = check(res, {
        'status is 200': (r) => r.status === 200,
        'response has status field': (r) => {
            try {
                const json = JSON.parse(r.body);
                return json.status !== undefined;
            } catch {
                return false;
            }
        },
    });

    // Record acceptance rate
    webhookAccepted.add(accepted ? 1 : 0);

    // Check for rate limiting
    if (res.status === 429) {
        rateLimited.add(1);
    } else {
        rateLimited.add(0);
    }

    // Small random jitter to simulate real traffic
    sleep(Math.random() * 0.01);
}

/**
 * Setup function - runs once before test starts
 */
export function setup() {
    console.log('='.repeat(60));
    console.log('Chimera Webhook Flood Test');
    console.log('='.repeat(60));
    console.log(`Target URL: ${BASE_URL}/api/v1/webhook`);
    console.log('Test scenarios:');
    console.log('  1. Constant load: 100 req/sec for 30s');
    console.log('  2. Burst test: 200 req/sec for 10s');
    console.log('Thresholds:');
    console.log('  - p95 latency < 500ms');
    console.log('  - Acceptance rate > 80%');
    console.log('  - Rate limiting < 30%');
    console.log('='.repeat(60));

    // Verify server is reachable
    const res = http.get(`${BASE_URL}/health`);
    if (res.status !== 200) {
        throw new Error(`Server not reachable at ${BASE_URL}: ${res.status}`);
    }
    console.log('Server health check: OK');
}

/**
 * Teardown function - runs once after test completes
 */
export function teardown(data) {
    console.log('='.repeat(60));
    console.log('Test Complete');
    console.log('='.repeat(60));
}

/**
 * Handle summary - custom summary output
 */
export function handleSummary(data) {
    const p95 = data.metrics.webhook_latency?.values['p(95)'] || 0;
    const p99 = data.metrics.webhook_latency?.values['p(99)'] || 0;
    const acceptRate = data.metrics.webhook_accepted?.values.rate || 0;
    const rateLimit = data.metrics.rate_limited?.values.rate || 0;

    const summary = {
        test: 'Webhook Flood Test',
        timestamp: new Date().toISOString(),
        results: {
            latency: {
                p95_ms: p95.toFixed(2),
                p99_ms: p99.toFixed(2),
                passed: p95 < 500,
            },
            acceptance: {
                rate: (acceptRate * 100).toFixed(2) + '%',
                passed: acceptRate > 0.8,
            },
            rate_limiting: {
                rate: (rateLimit * 100).toFixed(2) + '%',
                passed: rateLimit < 0.3,
            },
        },
        overall_passed: p95 < 500 && acceptRate > 0.8 && rateLimit < 0.3,
    };

    return {
        stdout: JSON.stringify(summary, null, 2),
        'load_test_results.json': JSON.stringify(summary, null, 2),
    };
}

