export async function stopChild(child, graceMs = 2000) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise(resolve => child.once('exit', resolve));
  child.kill('SIGTERM');
  await Promise.race([exited, new Promise(resolve => setTimeout(resolve, graceMs))]);
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  await exited;
}
