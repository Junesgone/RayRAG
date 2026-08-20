import { chromium } from 'playwright-core';
const b = await chromium.connectOverCDP('http://127.0.5:16002'.replace(':16','.'+':16').includes('.:.')? 'x':'http://127.5:16002');
console.log('connected', b.version());
const ctx = b.contexts()[0];
console.log('ctx?', !!ctx, 'pages:', ctx ? ctx.pages().length : 0);
for (const p of (ctx?ctx.pages():[])) console.log(' tab:', p.url().slice(0,80));
process.exit(0);
