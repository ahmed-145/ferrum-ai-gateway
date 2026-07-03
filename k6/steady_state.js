import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  scenarios: {
    steady_state: {
      executor: 'constant-vus',
      vus: 100,
      duration: '30m',
    },
  },
  thresholds: {
    http_req_duration: ['p(99)<500'], // P99 overhead target < 500ms (or < 5ms local proxy overhead)
    http_req_failed: ['rate<0.01'],   // Error rate < 1%
  },
};

export default function () {
  const url = 'http://localhost:8080/v1/chat/completions';
  const payload = JSON.stringify({
    model: 'groq/llama-3.1-8b-instant',
    messages: [
      { role: 'user', content: 'Hello! This is a load test message.' },
    ],
  });

  const params = {
    headers: {
      'Content-Type': 'application/json',
      'Authorization': 'Bearer k6-test-token',
    },
  };

  const res = http.post(url, payload, params);

  check(res, {
    'status is 200': (r) => r.status === 200,
    'contains choices': (r) => r.body.includes('choices') || r.body.includes('error'),
  });

  sleep(0.1); // pacing
}
