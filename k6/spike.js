import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  stages: [
    { duration: '10s', target: 500 },  // scale up
    { duration: '30s', target: 5000 }, // spike to 5,000 VUs
    { duration: '1m', target: 5000 },  // sustain
    { duration: '20s', target: 0 },    // scale down
  ],
  thresholds: {
    http_req_duration: ['p(99)<1000'], // P99 < 1s under high spike
    http_req_failed: ['rate<0.05'],
  },
};

export default function () {
  const url = 'http://localhost:8080/v1/chat/completions';
  const payload = JSON.stringify({
    model: 'groq/llama-3.1-8b-instant',
    messages: [
      { role: 'user', content: 'Spike load test request.' },
    ],
  });

  const params = {
    headers: {
      'Content-Type': 'application/json',
      'Authorization': 'Bearer k6-spike-token',
    },
  };

  const res = http.post(url, payload, params);

  check(res, {
    'status is 200': (r) => r.status === 200,
  });

  sleep(0.05);
}
