export async function activity(base, id, status, advertised) {
  const response = await fetch(`${base}/api/threads/${id}/activity`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ status }) });
  if (!response.ok) {
    const body = await response.text();
    // Older binaries may not advertise this route. A structured missing-thread
    // response still indicates a supported route, not an absent capability.
    let code;
    try { code = JSON.parse(body).error?.code; } catch { /* legacy non-JSON 404 */ }
    if (!advertised && (response.status === 404 || response.status === 405) && !code) return false;
    throw new Error(`activity: ${response.status} ${body}`);
  }
  return true;
}
