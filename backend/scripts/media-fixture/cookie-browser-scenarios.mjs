import { createHmac, createHash, randomBytes } from 'node:crypto';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { launchBrowser } from './browser-client.mjs';

const ACTOR = 1900000042;
const FOREIGN = 1900000099;
const BASE = '/api/v1/dl/cookies';
const sqlLiteral = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
const check = (ok, label) => { if (!ok) throw new Error(`Cookie browser assertion failed: ${label}`); };
const digest = value => createHash('sha256').update(value).digest('hex');

/** Telegram's WebAppData HMAC, not a dev token or fabricated JWT. */
export function signedInitData(token, actorId, now = Math.floor(Date.now() / 1000)) {
  const user = { id: actorId, first_name: 'Cookie Fixture', language_code: 'en' };
  const params = new URLSearchParams({ auth_date: String(now), query_id: 'fixture-cookie-browser', user: JSON.stringify(user), start_param: 'cookies' });
  const data = [...params].sort(([a], [b]) => a.localeCompare(b)).map(([k, v]) => `${k}=${v}`).join('\n');
  const secret = createHmac('sha256', 'WebAppData').update(token).digest();
  params.set('hash', createHmac('sha256', secret).update(data).digest('hex'));
  return params.toString();
}

/** Only records response status/route classes. Never records bodies, URLs or credentials. */
export function telegramBootstrap(initData, actorId) {
  return `(() => {
    localStorage.setItem('yoink_lang', 'en');
    const noop = () => {};
    const button = { setText(){return this}, show(){return this}, hide(){return this}, enable(){return this}, onClick:noop, offClick:noop, showProgress:noop, hideProgress:noop };
    window.Telegram = { WebApp: {
      initData: sessionStorage.getItem('fixture-cached-auth') ? '' : ${JSON.stringify(initData)},
      initDataUnsafe: {user:{id:${actorId},first_name:'Cookie Fixture',language_code:'en'},start_param:'cookies'},
      colorScheme:'dark', themeParams:{}, platform:'tdesktop', version:'9.0',
      ready(){window.__fixtureReady=true}, expand(){window.__fixtureExpanded=true},
      onEvent:noop, offEvent:noop, setHeaderColor:noop, setBackgroundColor:noop, disableVerticalSwipes:noop,
      enableClosingConfirmation:noop, disableClosingConfirmation:noop,
      MainButton:button, BackButton:button,
      HapticFeedback:{notificationOccurred:noop,selectionChanged:noop,impactOccurred:noop},
      CloudStorage:{getItem(k,cb){cb(null,k==='lang'?'en':null)},getItems(ks,cb){cb(null,{lang:'en'})},setItem(k,v,cb){cb?.(null,true)},removeItem(k,cb){cb?.(null,true)}},
      showConfirm(message,cb){cb(true)}
    }};
    window.__fixtureResponses=[];
    function record(url,status,text) {
      const path=new URL(url,location.href).pathname;
      let kind=path==='/api/v1/auth/token'?'auth':path==='${BASE}/yttv/start'?'start':path.startsWith('${BASE}/yttv/poll/')?'poll':path==='${BASE}/upload'?'upload':path.endsWith('/validate')?'validate':path==='/api/v1/dl/settings'?'settings':path==='${BASE}'?'list':null;
      if(!kind)return;
      let state=null,interval=null;
      if(kind==='poll'||kind==='start'){try{const data=JSON.parse(text);state=kind==='poll'?data.status:null;interval=kind==='start'?data.interval:null}catch{}}
      window.__fixtureResponses.push({kind,status,state,interval});
    }
    const fetchReal=window.fetch;
    window.fetch=async function(...args){const r=await fetchReal.apply(this,args); const u=typeof args[0]==='string'?args[0]:args[0].url; record(u,r.status,''); return r};
    const open=XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open=function(method,url,...args){this.addEventListener('loadend',()=>record(url,this.status,this.responseType===''||this.responseType==='text'?this.responseText:''),{once:true});return open.call(this,method,url,...args)};
  })();`;
}

/** Integration boundary: all methods operate the native fixture, never browser responses.
 * enable({actorId}) -> {enabled:true}; set({actorId,oauth,account});
 * counts({actorId}) -> {start,poll,validate}; restore() restores owned control state.
 * OAuth modes: pending, ok, expired, error, start_error. Account: valid, invalid, error.
 * Start responses must use interval=1, expire well after the first poll, and an inert
 * verification URL. Provider errors must use the native typed error contracts.
 */
export function providerAdapter(context) {
  const adapter = context.cookieBrowserProviders;
  for (const name of ['enable', 'set', 'counts', 'restore']) {
    if (typeof adapter?.[name] !== 'function') throw new Error('Cookie browser requires native offline provider control adapter');
  }
  return adapter;
}

export async function runCookieBrowserScenarios(context) {
  const { fixtureSql, waitFor, telegramToken, origin, certificateSpki, fixtureDir, registerCookieSyncToken } = context;
  check(new URL(origin).protocol === 'https:', 'owned HTTPS origin required');
  check(typeof registerCookieSyncToken === 'function', 'token diagnostic redaction required');
  const providers = providerAdapter(context);
  const scope = `user_id IN (${ACTOR},${FOREIGN})`;
  // Reserved actors are disposable and must not collide with pre-existing users.
  check(await fixtureSql(`SELECT count(*) FROM users WHERE id IN (${ACTOR},${FOREIGN})`) === '0', 'reserved fixture actors available');
  const snapshots = [];
  // Snapshot only the owned scope; generic column discovery keeps imported metadata intact.
  for (const table of ['cookies', 'dl_user_settings', 'user_permissions', 'cookie_web_tokens', 'cookie_oauth_sessions', 'runtime_personal_cookie_mutation_watermarks']) {
    const where = table === 'runtime_personal_cookie_mutation_watermarks' ? `actor_id IN (${ACTOR},${FOREIGN})` : scope;
    const columns = JSON.parse(await fixtureSql(`SELECT json_group_array(name) FROM pragma_table_info('${table}')`));
    const rows = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object(${columns.flatMap(c => [sqlLiteral(c), c]).join(',')})) FROM ${table} WHERE ${where}`));
    snapshots.push({ table, where, columns, rows });
  }
  let browser, directory, providerEnabled = false;
  let stage = 'bootstrap';
  const jar = (domain, value, httpOnly = false, name = 'sessionid') => `# Netscape HTTP Cookie File\n${httpOnly ? '#HttpOnly_' : ''}.${domain}\tTRUE\t/\tTRUE\t4102444800\t${name}\t${value}\n`;
  const secret = randomBytes(18).toString('hex');
  registerCookieSyncToken(secret);
  const ownedRows = async () => digest(await fixtureSql(`SELECT * FROM cookies WHERE ${scope} ORDER BY id`));
  const untouched = async () => digest(await fixtureSql(`SELECT * FROM cookies WHERE user_id NOT IN (${ACTOR},${FOREIGN}) ORDER BY id`));
  const baseline = await untouched();
  const otherSettings = () => fixtureSql(`SELECT * FROM dl_user_settings WHERE user_id NOT IN (${ACTOR},${FOREIGN}) ORDER BY user_id`).then(digest);
  const settingsBaseline = await otherSettings();
  const sqlCount = query => fixtureSql(`SELECT count(*) FROM cookies WHERE user_id=${ACTOR} AND is_pool=0 AND ${query}`);
  const evalBool = expression => browser.evaluate(`Boolean(${expression})`);
  const until = (expression, label) => waitFor(() => evalBool(expression), `cookie browser: ${label}`);
  const text = value => `document.body.innerText.includes(${JSON.stringify(value)})`;
  const responseCount = kind => browser.evaluate(`__fixtureResponses.filter(r=>r.kind===${JSON.stringify(kind)}).length`);
  async function response(kind, before, status, state) {
    try {
      await until(`__fixtureResponses.slice().filter(r=>r.kind===${JSON.stringify(kind)}).slice(${before}).some(r=>r.status===${status}${state ? `&&r.state===${JSON.stringify(state)}` : ''})`, `${kind} native status`);
    } catch {
      const statuses = await browser.evaluate(`__fixtureResponses.filter(r=>r.kind===${JSON.stringify(kind)}).slice(${before}).map(r=>Number(r.status))`);
      throw new Error(`Cookie browser assertion failed: ${stage}, ${kind} expected HTTP ${status}, observed ${JSON.stringify(statuses)}`);
    }
  }
  async function click(expression) {
    const point = await browser.evaluate(`(() => {const e=(${expression});if(!e||e.disabled)return null;e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
    check(point && point.x >= 0 && point.y >= 0, 'visible enabled UI target');
    await browser.request('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
    await browser.request('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
  }
  const button = label => `[...document.querySelectorAll('button')].find(e=>e.textContent.trim()===${JSON.stringify(label)})`;
  async function fill(selector, value) {
    await click(`document.querySelector(${JSON.stringify(selector)})`);
    await browser.request('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'a', code: 'KeyA', modifiers: 2, windowsVirtualKeyCode: 65, commands: ['selectAll'] });
    await browser.request('Input.dispatchKeyEvent', { type: 'keyUp', key: 'a', code: 'KeyA', modifiers: 2, windowsVirtualKeyCode: 65 });
    await browser.request('Input.insertText', { text: value });
  }
  const rowButton = (domain, icon) => `[...document.querySelectorAll('[data-slot="item"]')].find(e=>e.querySelector('[data-slot="item-title"]')?.textContent.trim()===${JSON.stringify(domain)})?.querySelector('button:has(svg[class*="lucide-${icon}"])')`;
  const rowVisible = domain => `[...document.querySelectorAll('[data-slot="item-title"]')].some(e=>e.textContent.trim()===${JSON.stringify(domain)})`;
  async function paste(content, domain, good = true) {
    stage = `paste ${domain}`;
    await click(button('Paste'));
    await fill('#paste-content', content);
    if (content.includes('#HttpOnly_') && !content.split('\n').some(line => line && !line.startsWith('#'))) {
      check(await evalBool('document.querySelector("#paste-domain").value===""'), 'HttpOnly-only jar requires manual domain');
    }
    await fill('#paste-domain', domain);
    check(await evalBool(`document.querySelector('#paste-content').value===${JSON.stringify(content)}`), 'browser text entry preserves cookie jar');
    check(await evalBool(`document.querySelector('#paste-domain').value===${JSON.stringify(domain)}`), 'browser text entry preserves cookie domain');
    const before = await responseCount('upload');
    await click(`document.querySelector('#paste-domain').parentElement.parentElement.querySelector('button:last-child')`);
    await response('upload', before, good ? 201 : 422);
    if (good) await until(`!document.querySelector('#paste-domain')&&${rowVisible(domain)}`, 'paste upload listed');
  }
  async function api(path, method = 'GET', body, authenticated = true) {
    // Security assertions only. Ordinary uploads/replacements/deletions use the UI.
    return browser.evaluate(`fetch(${JSON.stringify(path)},{method:${JSON.stringify(method)},headers:{'Content-Type':'application/json',${authenticated ? "Authorization:'Bearer '+localStorage.getItem('access_token')" : ''}},${body === undefined ? '' : `body:JSON.stringify(${JSON.stringify(body)}),`}}).then(r=>r.status)`);
  }
  try {
    await fixtureSql(`INSERT INTO users(id,role,language,theme,created_at,updated_at) VALUES (${ACTOR},'user','en','macchiato',unixepoch()*1000000,unixepoch()*1000000),(${FOREIGN},'user','en','macchiato',unixepoch()*1000000,unixepoch()*1000000)`);
    providerEnabled = true; // restore even if enable partially fails
    check((await providers.enable({ actorId: ACTOR })).enabled === true, 'native fake providers enabled');
    await providers.set({ actorId: ACTOR, oauth: 'pending', account: 'valid' });
    directory = await mkdtemp(join(fixtureDir, 'cookie-browser-'));
    const file = join(directory, 'instagram.com.txt');
    await writeFile(file, jar('instagram.com', secret), { mode: 0o600 });
    const initData = signedInitData(telegramToken, ACTOR);
    registerCookieSyncToken(initData);
    browser = await launchBrowser({ origin, certificateSpki, bootstrapScript: telegramBootstrap(initData, ACTOR) });
    await browser.navigate(`${origin}/cookies?tgWebAppStartParam=cookies`);
    await until(`${text('Paste')}&&localStorage.getItem('access_token')&&__fixtureReady&&__fixtureExpanded`, 'real SPA deep link bootstrap');
    await response('auth', 0, 200);
    const jwt = await browser.evaluate("localStorage.getItem('access_token')");
    check(typeof jwt === 'string' && jwt.length > 32, 'server-issued auth token available');
    registerCookieSyncToken(jwt);
    await response('settings', 0, 200);
    await response('list', 0, 200);
    check(await evalBool('location.pathname==="/cookies"'), 'deep link retained');
    await until(`!document.querySelector('[role="switch"]')`, 'ordinary user pool toggle hidden');
    // Reload without a live source exercises the real cached-token branch, in the same profile.
    await browser.evaluate("sessionStorage.setItem('fixture-cached-auth','1')");
    await browser.navigate(`${origin}/cookies`);
    await until(`${text('Paste')}&&localStorage.getItem('access_token')`, 'cached auth reload');
    await response('settings', 0, 200);
    await response('list', 0, 200);
    check(await responseCount('auth') === 0, 'cached reload does not exchange');

    stage = 'file upload';
    const { root } = await browser.request('DOM.getDocument');
    const { nodeId } = await browser.request('DOM.querySelector', { nodeId: root.nodeId, selector: 'input[type=file]' });
    await browser.request('DOM.setFileInputFiles', { nodeId, files: [file] });
    await until('document.querySelector("#upload-domain")?.value==="instagram.com"', 'file domain autodetection');
    const uploads = await responseCount('upload');
    await click(`document.querySelector('#upload-domain').parentElement.parentElement.querySelector('button:last-child')`);
    await response('upload', uploads, 201);
    await until(rowVisible('instagram.com'), 'file upload own list');
    check(await sqlCount("domain='instagram.com'") === '1', 'file persisted');
    const oldId = await fixtureSql(`SELECT id FROM cookies WHERE user_id=${ACTOR} AND domain='instagram.com' AND is_pool=0`);
    await paste(jar('instagram.com', `${secret}r`), 'instagram.com');
    check(await fixtureSql(`SELECT id FROM cookies WHERE user_id=${ACTOR} AND domain='instagram.com' AND is_pool=0`) === oldId, 'UI replacement retains ID');
    check(await fixtureSql(`SELECT content FROM cookies WHERE id=${oldId}`) === jar('instagram.com', `${secret}r`), 'UI replacement persisted');
    await paste(jar('twitter.com', secret, true, 'auth_token'), 'twitter.com');
    const beforeBad = await ownedRows();
    await paste(jar('youtube.com', secret, false, 'PREF'), 'youtube.com', false);
    check(await ownedRows() === beforeBad, 'unsuitable jar has no mutations');
    await click(button('Cancel'));

    for (const mode of ['valid', 'invalid', 'error']) {
      await providers.set({ actorId: ACTOR, account: mode, oauth: 'pending' });
      const calls = await providers.counts({ actorId: ACTOR });
      const before = await responseCount('validate');
      const rows = await ownedRows();
      await click(rowButton('instagram.com', 'circle-check'));
      await response('validate', before, mode === 'error' ? 502 : 200);
      await until(text(mode === 'valid' ? 'Cookie is valid' : mode === 'invalid' ? 'Cookie appears invalid' : 'Validation failed'), 'account validation UI outcome');
      await waitFor(async () => (await providers.counts({ actorId: ACTOR })).validate > calls.validate, 'native account provider called');
      if (mode === 'error') check(await ownedRows() === rows, 'provider error preserves rows');
      else check(await fixtureSql(`SELECT is_valid FROM cookies WHERE id=${oldId}`) === (mode === 'valid' ? '1' : '0'), 'native validation state');
    }
    await click(rowButton('instagram.com', 'trash-2'));
    await waitFor(async () => await sqlCount("domain='instagram.com'") === '0', 'UI own deletion persisted');
    await until(`!(${rowVisible('instagram.com')})`, 'deleted row removed from UI');

    await click(`[...document.querySelectorAll('[aria-expanded]')].find(e=>e.textContent.includes('Auto-sync'))`);
    await click(button('Generate'));
    await until('document.querySelector("code")', 'sync token generated');
    const token = await browser.evaluate('document.querySelector("code").textContent.trim()');
    registerCookieSyncToken(token);
    check(typeof token === 'string' && token.length > 32, 'token shape');
    check(await evalBool('isSecureContext&&navigator.clipboard'), 'secure clipboard available');
    await browser.browserRequest('Browser.grantPermissions', { origin, permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite'] });
    await click(`document.querySelector('button:has(svg.lucide-copy)')`);
    await until(text('Copied!'), 'UI clipboard copy');
    check(await browser.evaluate('navigator.clipboard.readText()') === token, 'actual clipboard token equality');
    const submission = { token, cookies: { 'youtube.com': [{ domain: '.youtube.com', path: '/', name: 'SID', value: secret, secure: true, httpOnly: true, expirationDate: 4102444800 }] } };
    check(await api(`${BASE}/submit`, 'POST', submission, false) === 204, 'JWT-free submit accepted');
    const afterSubmit = await ownedRows();
    check(await api(`${BASE}/submit`, 'POST', submission, false) === 401, 'submit replay rejected');
    check(await ownedRows() === afterSubmit, 'replay does not mutate');

    // Fixture foreign/pool rows are seeded, never replaced or removed through UI.
    await fixtureSql(`INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES (${FOREIGN},'foreign.test',${sqlLiteral(jar('foreign.test',secret))},1,0,unixepoch()*1000000,unixepoch()*1000000),(${FOREIGN},'browser-fixture-pool.test',${sqlLiteral(jar('browser-fixture-pool.test',secret))},1,1,unixepoch()*1000000,unixepoch()*1000000)`);
    const foreignId = await fixtureSql(`SELECT id FROM cookies WHERE user_id=${FOREIGN} AND domain='foreign.test'`);
    const foreignState = () => fixtureSql(`SELECT * FROM cookies WHERE user_id=${FOREIGN} ORDER BY id`).then(digest);
    const foreignBefore = await foreignState();
    const deniedProviderCounts = JSON.stringify(await providers.counts({ actorId: ACTOR }));
    check(await api(`${BASE}/by-id/${foreignId}`, 'DELETE', { user_id: FOREIGN }) === 404, 'forged foreign deletion denied');
    check(await api(`${BASE}/${foreignId}/validate`, 'POST', { user_id: FOREIGN }) === 403, 'forged foreign validation denied');
    check(await foreignState() === foreignBefore, 'foreign mutations preserve data');
    check(JSON.stringify(await providers.counts({ actorId: ACTOR })) === deniedProviderCounts, 'foreign denial never calls providers');
    await fixtureSql(`UPDATE users SET role='owner' WHERE id=${ACTOR}`);
    await browser.navigate(`${origin}/cookies`);
    await until(`document.querySelector('[role="switch"]')&&${rowVisible('browser-fixture-pool.test')}`, 'owner inherited pool visible');
    await until('document.querySelector("[role=switch]")&&!document.querySelector("[role=switch]").disabled', 'pool toggle enabled');
    await click(`document.querySelector('[role="switch"]')`);
    await waitFor(async () => await fixtureSql(`SELECT use_pool_cookies FROM dl_user_settings WHERE user_id=${ACTOR}`) === '0', 'UI pool toggle persisted off');
    await until('document.querySelector("[role=switch]")&&!document.querySelector("[role=switch]").disabled', 'pool toggle enabled');
    await click(`document.querySelector('[role="switch"]')`);
    await waitFor(async () => await fixtureSql(`SELECT use_pool_cookies FROM dl_user_settings WHERE user_id=${ACTOR}`) === '1', 'UI pool toggle persisted on');
    await fixtureSql(`UPDATE users SET role='user' WHERE id=${ACTOR}`);
    await fixtureSql(`INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES (${ACTOR},'dl','shared_cookies',${ACTOR},unixepoch()*1000000)`);
    await browser.navigate(`${origin}/cookies`);
    await until(`document.querySelector('[role="switch"]')&&${rowVisible('browser-fixture-pool.test')}`, 'authorized user inherited pool visible');
    await fixtureSql(`DELETE FROM user_permissions WHERE user_id=${ACTOR} AND plugin='dl' AND feature='shared_cookies'`);
    await browser.navigate(`${origin}/cookies`);
    await until(`!document.querySelector('[role="switch"]')&&!(${rowVisible('browser-fixture-pool.test')})&&${text('Paste')}`, 'grant revocation hides inherited pool');
    check(await evalBool(`localStorage.getItem('access_token')===${JSON.stringify(jwt)}`), 'grant revocation uses unchanged cached JWT');
    await fixtureSql(`UPDATE users SET role='banned' WHERE id=${ACTOR}`);
    const revokedBefore = await ownedRows();
    check(await api(`${BASE}/by-id/${foreignId}`, 'DELETE') === 403, 'revoked foreign mutation denied');
    check(await api(`${BASE}/pool`, 'POST', { domain: 'browser-fixture-pool.test', content: jar('browser-fixture-pool.test', `${secret}revoked`) }) === 403, 'revoked pool mutation denied');
    check(await ownedRows() === revokedBefore && await foreignState() === foreignBefore, 'revocation preserves foreign and pool data');
    await fixtureSql(`UPDATE users SET role='user' WHERE id=${ACTOR}`);
    await browser.navigate(`${origin}/cookies`);
    await until(text('Paste'), 'authority restored');

    await click(`[...document.querySelectorAll('[aria-expanded]')].find(e=>e.textContent.includes('Authorize via YouTube TV'))`);
    for (const mode of ['pending', 'ok', 'expired', 'error', 'start_error']) {
      await providers.set({ actorId: ACTOR, oauth: mode, account: 'valid' });
      const calls = await providers.counts({ actorId: ACTOR });
      const rowsBeforeFlow = await ownedRows();
      const starts = await responseCount('start');
      const polls = await responseCount('poll');
      await click(button('Start authorization'));
      await response('start', starts, mode === 'start_error' ? 502 : 200);
      await waitFor(async () => (await providers.counts({ actorId: ACTOR })).start > calls.start, 'native OAuth start provider called');
      if (mode === 'start_error') {
        await until(text('Failed to start authorization'), 'TV start rejection UI');
        check(await ownedRows() === rowsBeforeFlow, 'start rejection does not mutate cookies');
      } else {
        await until(text('Waiting for authorization...'), 'TV device flow displayed');
        check(await evalBool('__fixtureResponses.filter(r=>r.kind==="start").at(-1).interval===1'), 'fixture provider one-second poll interval');
        await response('poll', polls, 200, mode);
        await waitFor(async () => (await providers.counts({ actorId: ACTOR })).poll > calls.poll, 'native OAuth poll provider called');
        if (mode === 'pending') {
          const rows = await ownedRows();
          await click(button('Cancel'));
          await until(`!(${text('Waiting for authorization...')})&&${button('Start authorization')}`, 'TV cancel UI');
          const count = await responseCount('poll');
          await sleep(2500);
          check(await responseCount('poll') === count && await ownedRows() === rows, 'cancel stops polling without cookie writes');
        } else {
          await until(text(mode === 'ok' ? 'YouTube authorized successfully' : mode === 'expired' ? 'Authorization expired' : 'Provider authentication failed'), `TV ${mode} UI`);
          if (mode === 'ok') {
            await waitFor(async () => await sqlCount("domain='youtube.com' AND content LIKE '%__oauth2__%'") === '1', 'native OAuth credential persisted');
            await until(text('OAuth'), 'OAuth credential appears in own list');
          } else check(await ownedRows() === rowsBeforeFlow, 'failed TV flow does not mutate cookies');
        }
      }
    }
    if (context.botApi) {
      const before = context.botApi.calls.length;
      context.botApi.enqueue(ACTOR, ACTOR, '/cookie token', 1900000042);
      const reply = await waitFor(() => context.botApi.calls.slice(before).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === ACTOR), 'cookie browser Telegram link');
      const issued = reply.data.text.split('\n')[2];
      if (typeof issued === 'string' && /^[A-Za-z0-9_-]{32,128}$/.test(issued)) registerCookieSyncToken(issued);
      const links = reply.data.reply_markup?.inline_keyboard?.flat() ?? [];
      check(links.some(b => b.text === 'Manage cookies' && b.web_app?.url === new URL('/cookies', origin).href), 'Telegram cookie HTTPS WebApp deep link');
    }
    check(await untouched() === baseline, 'nonfixture cookie rows unchanged');
    check(await otherSettings() === settingsBaseline, 'nonfixture settings unchanged');
    return { browser: 'real React HTTPS SPA', auth: ['live Telegram exchange', 'cached reload', 'deep link'], uploads: ['file', 'paste', 'HttpOnly manual domain', 'unsuitable rejection', 'replace', 'delete'], validation: ['valid', 'invalid', 'error'], sync: ['secure clipboard', 'JWT-free submit', 'replay rejected'], authority: ['ordinary hidden', 'owner pool toggle', 'authorized inheritance', 'foreign denied', 'revoked denied'], tv: ['pending', 'cancel', 'ok', 'expired', 'error', 'start_error'] };
  } catch (error) {
    if (error instanceof Error && (error.message.startsWith('Cookie browser assertion failed:') || error.message.startsWith('timeout waiting for cookie browser:'))) throw error;
    // SQL/provider/DOM errors may contain sensitive data; do not propagate causes.
    throw new Error('Cookie browser scenario execution failed');
  } finally {
    // Close first: no outstanding React polling may race fixture restoration.
    // Attempt every owned cleanup even when an earlier one fails; never expose causes.
    let cleanupFailed = false;
    const attempt = async action => { try { await action(); } catch { cleanupFailed = true; } };
    await attempt(() => browser?.close());
    if (providerEnabled) await attempt(() => providers.restore());
    for (const { table, where, columns, rows } of snapshots) {
      await attempt(() => fixtureSql(`DELETE FROM ${table} WHERE ${where}`));
      for (const row of rows) await attempt(() => fixtureSql(`INSERT INTO ${table}(${columns.join(',')}) VALUES (${columns.map(c => sqlLiteral(row[c])).join(',')})`));
    }
    await attempt(() => fixtureSql(`DELETE FROM users WHERE id IN (${ACTOR},${FOREIGN})`));
    if (directory) await attempt(() => rm(directory, { recursive: true, force: true }));
    if (cleanupFailed) throw new Error('Cookie browser fixture cleanup failed');
  }
}
