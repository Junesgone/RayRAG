import { chromium } from 'playwright-core';
const RBASE='http://127.0.0.1:9390';
setTimeout(()=>{console.log('WATCHDOG');process.exit(9)},8*60*1000).unref?.();
const b=await chromium.connectOverCDP('http://127.0.0.1:16002',{timeout:60000});
const ctx=b.contexts()[0];
let page=(ctx.pages().find(p=>p.url().startsWith(RBASE)))||(await ctx.newPage());
page.on('dialog',d=>{try{d.dismiss()}catch(e){}});

// --- /agents: + new agent button & card buttons wiring
await page.goto(RBASE+'/agents',{waitUntil:'domcontentloaded',timeout:45000});
await new Promise(r=>setTimeout(r,1800));
const agents=await page.evaluate(()=>({
  btn:[...document.querySelectorAll('button')].filter(x=>/新建智能体|New Agent/.test((x.innerText||''))).map(b=>({oc:b.getAttribute('onclick'),html:b.outerHTML.slice(0,160)})),
  modal:(document.querySelector('[id*=gentModal]')||{}).id||null,
  cardBtns:[...document.querySelectorAll('.agent-card button, [class*=card] button')].slice(0,8).map(x=>(x.innerText||'').trim()+' :: '+(x.getAttribute('onclick')||'<none>')),
}));
console.log('AGENTS',JSON.stringify(agents,null,1).slice(0,1200));

// --- /user-setting/profile: header nav wiring
await page.goto(RBASE+'/user-setting/profile',{waitUntil:'domcontentloaded',timeout:45000});
await new Promise(r=>setTimeout(r,1800));
const us=await page.evaluate(()=>({
  links:[...document.querySelectorAll('a[href]')].filter(a=>/Search|Files|Skills|Memories/.test((a.innerText||''))).map(a=>({t:(a.innerText||'').trim().slice(0,20),h:a.getAttribute('href'),oc:a.getAttribute('onclick')})),
}));
console.log('USLINKS',JSON.stringify(us,null,1).slice(0,900));
process.exit(0);
