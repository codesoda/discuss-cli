const ms = (start, end) => Number(end - start) / 1e6;

export function timingSamples(result) {
  const submissionToVisibleMs = ms(result.sentAt, result.visibleAt);
  const actualTimerMs = ms(result.waitStart, result.waitEnd);
  const queueWaitMs = ms(result.receivedAt, result.handleStart);
  return {
    submissionToVisibleMs,
    observedWaitMs: ms(result.receivedAt, result.postAt),
    actualTimerMs,
    queueWaitMs,
    indicatorMs: Number(result.indicatorNs) / 1e6,
    overheadMs: submissionToVisibleMs - actualTimerMs - queueWaitMs
  };
}
