import http from 'k6/http';
import { check, sleep } from 'k6';
import { Trend, Rate, Counter } from 'k6/metrics';

// Custom metrics for production simulation
const productionSignalRate = new Rate('production_signal_rate');
const walletDiversity = new Trend('wallet_diversity');
const strategyDistribution = new Counter('strategy_distribution', ['strategy']);
const signalTypeDistribution = new Counter('signal_type_distribution', ['action']);

// Production wallet signal templates based on actual Chimera wallet behavior
const WALLET_TEMPLATES = {
    alpha_hunter: {
        name: 'Alpha Hunter',
        strategies: ['SHIELD', 'SPEAR'],
        avg_amount_sol: 0.15,
        min_amount_sol: 0.05,
        max_amount_sol: 0.3,
        signal_frequency: 'high',
        buy_sell_ratio: 0.7, // 70% BUY, 30% SELL
    },
    conservative_copier: {
        name: 'Conservative Copier',
        strategies: ['SHIELD'],
        avg_amount_sol: 0.08,
        min_amount_sol: 0.02,
        max_amount_sol: 0.15,
        signal_frequency: 'medium',
        buy_sell_ratio: 0.6,
    },
    swing_trader: {
        name: 'Swing Trader',
        strategies: ['SHIELD', 'EXIT'],
        avg_amount_sol: 0.12,
        min_amount_sol: 0.05,
        max_amount_sol: 0.25,
        signal_frequency: 'low',
        buy_sell_ratio: 0.5,
    },
    exit_specialist: {
        name: 'Exit Specialist',
        strategies: ['EXIT'],
        avg_amount_sol: 0.1,
        min_amount_sol: 0.02,
        max_amount_sol: 0.2,
        signal_frequency: 'medium',
        buy_sell_ratio: 0.1, // Mostly SELL/EXIT
    },
    spear_trader: {
        name: 'Spear Trader',
        strategies: ['SPEAR'],
        avg_amount_sol: 0.2,
        min_amount_sol: 0.1,
        max_amount_sol: 0.4,
        signal_frequency: 'high',
        buy_sell_ratio: 0.8,
    },
};

// Token distribution based on production signal patterns
const TOKENS = ['SOL', 'BONK', 'WIF', 'POPCAT', 'RAY', 'JUP', 'ORCA', 'MNGO', 'SAMO'];

// Load test scenarios
export const options = {
    scenarios: {
        // Scenario 1: Steady-state production load
        steady_state: {
            executor: 'constant-arrival-rate',
            rate: 25, // 25 signals/sec (~2000/day per active wallet)
            timeUnit: '1s',
            duration: '10m',
            preAllocatedVUs: 50,
            gracefulStop: '30s',
        },
        // Scenario 2: Alpha Hunter surge (new token discovery)
        alpha_hunter_surge: {
            executor: 'ramping-arrival-rate',
            startRate: 10,
            timeUnit: '1s',
            preAllocatedVUs: 100,
            gracefulStop: '30s',
            stages: [
                { duration: '2m', target: 50 },   // Ramp to 50 req/s
                { duration: '3m', target: 100 },  // Alpha hunter discovery spike
                { duration: '2m', target: 50 },   // Cool down
                { duration: '3m', target: 25 },   // Return to baseline
            ],
        },
        // Scenario 3: Market crash simulation (mass exit signals)
        market_crash: {
            executor: 'constant-arrival-rate',
            rate: 100, // Sudden exit surge
            timeUnit: '1s',
            duration: '30s',
            preAllocatedVUs: 200,
            startTime: '15m', // Starts after steady_state completes
            gracefulStop: '10s',
        },
        // Scenario 4: Sustained high load (stress test)
        sustained_load: {
            executor: 'constant-arrival-rate',
            rate: 40, // Higher sustained load
            timeUnit: '1s',
            duration: '5m',
            preAllocatedVUs: 80,
            startTime: '20m',
            gracefulStop: '30s',
        },
    },
    thresholds: {
        'http_req_duration': ['p(95)<500', 'p(99)<1000'],
        'http_req_failed': ['rate<0.05'], // Less than 5% failure rate
        'production_signal_rate': ['rate>0.90'], // 90% acceptance rate
        'checks': ['rate>0.95'],
    },
};

// Helper function to generate UUID
function uuidv4() {
    return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function(c) {
        const r = Math.random() * 16 | 0;
        const v = c === 'x' ? r : (r & 0x3 | 0x8);
        return v.toString(16);
    });
}

// Helper function to generate HMAC signature
function generateHMAC(payload, secret) {
    // Note: In production, this would use crypto.subtle or a compatible HMAC function
    // For load testing, we're using a simplified version
    const crypto = require('k6/crypto');
    const hasher = crypto.createHMAC('sha256', secret || __ENV.WEBHOOK_SECRET || 'test-secret');
    hasher.update(payload);
    return hasher.digest('hex');
}

function pickRandomWalletType() {
    const types = Object.keys(WALLET_TEMPLATES);
    // Weight selection towards more common wallet types
    const weights = [0.35, 0.30, 0.20, 0.10, 0.05]; // Alpha Hunter most common
    const random = Math.random();
    let cumulative = 0;

    for (let i = 0; i < types.length; i++) {
        cumulative += weights[i];
        if (random <= cumulative) {
            return types[i];
        }
    }
    return types[0]; // Default to alpha_hunter
}

function pickRandomToken() {
    return TOKENS[Math.floor(Math.random() * TOKENS.length)];
}

function generateProductionSignal(walletType) {
    const template = WALLET_TEMPLATES[walletType];
    const strategy = template.strategies[Math.floor(Math.random() * template.strategies.length)];

    // Determine action based on buy_sell_ratio
    const isBuy = Math.random() < template.buy_sell_ratio;
    const action = strategy === 'EXIT' ? 'SELL' : (isBuy ? 'BUY' : 'SELL');

    // Calculate amount with some variance
    const variance = 0.8 + (Math.random() * 0.4); // 0.8 to 1.2
    const amount = (template.avg_amount_sol * variance).toFixed(4);

    // Get current timestamp
    const timestamp = new Date().toISOString();

    return {
        trade_uuid: uuidv4(),
        wallet_address: __ENV.TEST_WALLET || 'test_wallet_' + walletType + '_' + Date.now().toString(),
        token: pickRandomToken(),
        action: action,
        strategy: strategy,
        amount_sol: parseFloat(amount),
        timestamp: timestamp,
    };
}

export default function(data) {
    // Simulate realistic production wallet behavior
    const walletType = pickRandomWalletType();
    const signal = generateProductionSignal(walletType);

    const payload = JSON.stringify(signal);
    const signature = generateHMAC(payload);
    const timestamp = Math.floor(Date.now() / 1000).toString();

    const response = http.post(
        __ENV.WEBHOOK_URL || 'http://localhost:3000/api/v1/webhook',
        payload,
        {
            headers: {
                'Content-Type': 'application/json',
                'X-Signature': signature,
                'X-Timestamp': timestamp,
            },
            tags: {
                wallet_type: walletType,
                strategy: signal.strategy,
                action: signal.action,
            },
        }
    );

    // Track wallet diversity
    const walletTypeIndex = Object.keys(WALLET_TEMPLATES).indexOf(walletType);
    walletDiversity.add(walletTypeIndex);

    // Track strategy distribution
    strategyDistribution.add(1, { strategy: signal.strategy });

    // Track signal type distribution
    signalTypeDistribution.add(1, { action: signal.action });

    // Validate response
    const checkSuccess = check(response, {
        'status accepted': (r) => r.status === 202 || r.status === 200,
        'signal queued or executing': (r) => {
            try {
                const body = r.json();
                return body.status === 'QUEUED' || body.status === 'EXECUTING' || body.status === 'PENDING';
            } catch {
                return false;
            }
        },
        'no rate limit': (r) => r.status !== 429,
        'no server error': (r) => r.status !== 500 && r.status !== 503,
    });

    productionSignalRate.record(checkSuccess);

    // Realistic inter-signal delay based on wallet type
    const delays = {
        alpha_hunter: 1,
        spear_trader: 2,
        conservative_copier: 3,
        swing_trader: 4,
        exit_specialist: 2,
    };
    sleep(delays[walletType] || 2);
}

export function handleSummary(data) {
    // Custom summary output
    console.log('\n=== Production Wallet Simulation Summary ===');
    console.log('Total requests:', data.metrics.http_reqs_total.values);
    console.log('Failed requests:', data.metrics.http_req_failed.values);
    console.log('Signal acceptance rate:', data.metrics.production_signal_rate.values);
    console.log('Wallet diversity (unique types):', new Set(data.metrics.wallet_diversity.values).size);
}

export function setup() {
    // Setup: Validate environment and configuration
    console.log('=== Production Wallet Simulation Setup ===');
    console.log('WEBHOOK_URL:', __ENV.WEBHOOK_URL || 'http://localhost:3000/api/v1/webhook');
    console.log('TEST_WALLET:', __ENV.TEST_WALLET || 'default_test_wallet');

    // If PRODUCTION_SIGNALS_FILE is provided, load production signal patterns
    if (__ENV.PRODUCTION_SIGNALS_FILE) {
        console.log('Loading production signal patterns from:', __ENV.PRODUCTION_SIGNALS_FILE);
        try {
            const signals = open(__ENV.PRODUCTION_SIGNALS_FILE);
            console.log('Loaded signals file successfully');
        } catch (e) {
            console.error('Failed to load signals file:', e);
        }
    }

    return {
        startTime: new Date().toISOString(),
        config: {
            webhookUrl: __ENV.WEBHOOK_URL || 'http://localhost:3000/api/v1/webhook',
            testWallet: __ENV.TEST_WALLET || 'default_test_wallet',
        },
    };
}

export function teardown(data) {
    console.log('\n=== Production Wallet Simulation Teardown ===');
    console.log('Test started at:', data.startTime);
    console.log('Test completed at:', new Date().toISOString());
}
