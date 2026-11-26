export async function launchBrowser() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
  return chromium.launch({ executablePath: process.env.CHROME_BIN, args: ['--no-sandbox'], headless: true });
}
