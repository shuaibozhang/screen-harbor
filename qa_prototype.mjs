import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = require('C:/Users/Administrator/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright');

const browser = await chromium.launch({ headless: true, executablePath: 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe' });
for (const width of [736]) {
  const page = await browser.newPage({ viewport: { width, height: 920 }, deviceScaleFactor: 1 });
  await page.goto('file:///C:/Users/Administrator/Desktop/cut/prototype-preview.html');
  await page.waitForTimeout(500);
  const frame = page.frames().find(item => item !== page.mainFrame());
  const metrics = await frame.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
    buttons: document.querySelectorAll('button').length,
    state: document.querySelector('.ff-desktop')?.getAttribute('data-current-state'),
    desktopHeight: document.querySelector('.ff-desktop')?.getBoundingClientRect().height
  }));
  console.log(width, JSON.stringify(metrics));
  await frame.locator('button[data-state="settings"]').click();
  await page.waitForTimeout(80);
  await page.screenshot({ path: `C:/Users/Administrator/Desktop/cut/qa/prototype-settings-${width}.png`, fullPage: true });
  await frame.locator('#ff-preview-toggle').uncheck();
  const summary = await frame.locator('.ff-output-summary strong').textContent();
  const hidden = await frame.locator('[data-dependent="preview"]').evaluate(node => getComputedStyle(node).display === 'none');
  if (!summary.includes('主视频') || !hidden) throw new Error('Preview toggle dependency failed');
  await frame.locator('#ff-cover-toggle').uncheck();
  const onlyMaster = await frame.locator('.ff-output-summary strong').textContent();
  if (onlyMaster !== '仅主视频') throw new Error('Cover toggle dependency failed');
}
await browser.close();
