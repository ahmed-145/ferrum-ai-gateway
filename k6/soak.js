import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  scenarios: {
    soak: {
      executor: 'constant-vus',
      vus: 50,
      duration: '72h',
    },
  },
  thresholds: {
    http_req_duration: ['p(95)<300'],
    http_req_failed: ['rate<0.001'], // 0.1% max failure rate over 72h
  },
};

export default function () {
  const url = 'http://localhost:8080/v1/chat/completions';
  const payload = JSON.stringify({
    model: 'groq/llama-3.1-8b-instant',
    messages: [
      { role: 'user', content: 'Soak test message.' },
    ],
  });

  const params = {
    headers: {
      'Content-Type': 'application/json',
      'Authorization': 'Bearer k6-soak-token',
    },
  };

  const res = http.post(url, payload, params);

  check(res, {
    'status is 200': (r) => r.status === 200,
  });

  sleep(0.5); // moderate pacing
}
