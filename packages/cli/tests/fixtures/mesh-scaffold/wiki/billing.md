---
title: Billing
summary: How charges flow from the browser through the server to Stripe.
---

# Billing

The billing service validates the checkout payload before [submitCheckout](src/checkout.ts#L2-L8) is called.

## Charge handler

The server-side handler [handleCharge](src/charge.ts#L2-L7) validates the schema and dispatches to Stripe.

The checkout payload schema is shared between browser and server.
