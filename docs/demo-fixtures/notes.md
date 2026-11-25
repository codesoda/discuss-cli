# Provider Brownout Notes

The March incident lasted 41 minutes. Retries during the brownout window would have recovered an estimated 84% of failed charges, based on replayed traffic.

The provider rate-limits at 50 requests per second per merchant. Retry bursts must stay inside that budget or we trade one failure mode for another.
