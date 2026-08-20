import { chromium } from 'playwright-core';
const log=(m)=>console.log(new Date().toISOString(),m);
setTimeout(()=>{log('WATCHDOG');process.exit(9)},10*60*1000).unref?.();
try{
 log('connecting');
 const b=await chromium.connectOverCDP('http://127.0.0.1:16002');
 log('connected '+b.version());
 const ctx=b.contexts()[0];
 log('ctx='+(!!ctx)+' pages='+(ctx?ctx.pages().length:0));
 for(const p of (ctx?ctx.pages():[])) log('tab: '+p.url().slice(0,90));
 const page=(ctx.pages().find(p=>p.url()==='about:blank'))||(await ctx.newPage());
 await page.goto('http://127.0.0.1:9390/login',{waitUntil:'domcontentloaded',timeout:45000});
 log('title='+await page.title());
 process.exit(0);
}catch(e){log('ERROR '+String(e).slice(0,250));process.exit(8)}
