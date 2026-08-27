---
title: Payments Retry Plan
owner: platform-team
status: draft
reviewers: [chris, dana]
---

# Payments Retry Plan

We see a 2.1% failure rate on charge attempts during provider brownouts. Most failures are transient. A bounded retry with jitter recovers the majority without risking double charges.

This plan adds an idempotent retry pipeline in front of the provider client. It ships behind a feature flag and rolls out per merchant.

## Design

Retries are safe because every charge attempt carries an idempotency key. The provider deduplicates on that key, so a retry can never double-charge.

```rust
pub async fn charge_with_retry(req: ChargeRequest) -> Result<Receipt, ChargeError> {
    let key = req.idempotency_key.clone();
    retry::with_backoff(3, Jitter::Full, || client.charge(&req, &key)).await
}
```

```mermaid
flowchart LR
    A[Charge request] --> B{Provider up?}
    B -- yes --> C[Charge once]
    B -- no --> D[Retry with jitter]
    D --> C
    C --> E[Receipt]
```

## Rollout

| Stage | Merchants | Flag | Exit criteria |
|-------|-----------|------|---------------|
| 1 | Internal test | `retry_v2=on` | Zero duplicate receipts in 48 h |
| 2 | 5% cohort | `retry_v2=on` | Failure rate under 0.5% |
| 3 | All | default on | Two clean weeks |

Rollback is a flag flip. No data migration is needed at any stage.
