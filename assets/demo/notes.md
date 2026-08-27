# Provider Brownout Notes

The March incident lasted 41 minutes. Retries during the brownout window would have recovered an estimated 79% of failed charges, based on replayed traffic. The estimate dropped from an earlier 84% figure because the replay now excludes attempts that were rejected with `429 Too Many Requests` — those would have been rate-limited again, not recovered.

The provider rate-limits at 50 requests per second per merchant. Retry bursts must stay inside that budget or we trade one failure mode for another.

To keep first-attempt traffic safe, the retry pipeline reserves at most 20% of the 50 rps limit for retries. First attempts always get the remaining 80%, so a retry storm can slow recovery but can never crowd out new charges.
