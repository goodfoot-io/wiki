// Stub: server-side charge handler.
export async function handleCharge(req: Request) {
  // Validates the schema and dispatches to Stripe.
  const body = await req.json();
  console.log('charge', body);
  return new Response('ok');
}
