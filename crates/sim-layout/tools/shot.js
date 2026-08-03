const { chromium } = require('playwright-core');
const path = require('path');
const fs = require('fs');
(async () => {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const browser = await chromium.launch({ channel: 'chrome', args: ['--disable-gpu'] });
      const page = await browser.newPage();
      for (const f of process.argv.slice(2)) {
        const svg = fs.readFileSync(f, 'utf8');
        const m = svg.match(/width="(\d+)" height="(\d+)"/);
        const w = Math.min(2000, +m[1]), h = Math.min(2400, +m[2]);
        await page.setViewportSize({ width: w, height: h });
        await page.setContent(`<body style="margin:0">${svg}</body>`);
        await page.waitForTimeout(300);
        await page.screenshot({ path: f.replace('.svg', '.png'), clip: { x: 0, y: 0, width: w, height: h } });
        console.error('shot', f);
      }
      await browser.close();
      return;
    } catch (e) {
      console.error('attempt', attempt, e.message);
      if (attempt === 2) process.exit(1);
    }
  }
})();
