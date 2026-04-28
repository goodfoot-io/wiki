// Stub: checkout request flow.
export function submitCheckout(payload: unknown) {
  // POST to /api/charge with the validated payload.
  return fetch('/api/charge', {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}
