import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = require('C:/Users/Administrator/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright');

const browser = await chromium.launch({ headless: true, executablePath: 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe' });
for (const width of [736, 420]) {
  const page = await browser.newPage({ viewport: { width, height: 1000 }, deviceScaleFactor: 1 });
  await page.goto('file:///C:/Users/Administrator/Desktop/cut/prototype-unified-preview.html');
  await page.waitForTimeout(500);
  const frame = page.frames().find(item => item !== page.mainFrame());
  const metrics = await frame.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
    buttons: document.querySelectorAll('button').length,
    state: document.querySelector('.ffu-scene')?.getAttribute('data-state')
  }));
  console.log(width, JSON.stringify(metrics));
  if (metrics.scrollWidth > metrics.clientWidth) throw new Error(`Horizontal overflow at ${width}px`);
  await frame.locator('#ffu-preview-format').selectOption({ label: '动态 WebP' });
  const audioHidden = await frame.locator('#ffu-preview-audio-row').evaluate(node => getComputedStyle(node).display === 'none');
  const noteVisible = await frame.locator('#ffu-image-note').evaluate(node => getComputedStyle(node).display !== 'none');
  const summary = await frame.locator('#ffu-summary').textContent();
  if (!audioHidden || !noteVisible || !summary.includes('动态 WebP') || !summary.includes('无声音')) throw new Error('Animated preview dependency failed');
  await frame.locator('#ffu-main-audio').evaluate(node => { node.checked = false; node.dispatchEvent(new Event('change', { bubbles: true })); });
  const mainSummary = await frame.locator('#ffu-summary').textContent();
  if (!mainSummary.startsWith('MP4 · 无声音')) throw new Error('Main audio toggle failed');
  await page.screenshot({ path: `C:/Users/Administrator/Desktop/cut/qa/unified-output-${width}.png`, fullPage: true });
}
await browser.close();
