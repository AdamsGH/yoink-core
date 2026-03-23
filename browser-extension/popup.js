'use strict';

const DEFAULT_DOMAINS = ['youtube.com', 'tiktok.com', 'instagram.com', 'x.com'];

let domains = [];

// ── Persist / load ────────────────────────────────────────────────────────────

async function load() {
  const data = await chrome.storage.sync.get(['botUrl', 'domains']);
  document.getElementById('bot-url').value = data.botUrl || '';
  domains = data.domains || [...DEFAULT_DOMAINS];
  renderDomains();
  updateCurrentSite();
}

async function savePersist() {
  await chrome.storage.sync.set({
    botUrl: document.getElementById('bot-url').value.trim().replace(/\/$/, ''),
    domains,
  });
}

// ── UI helpers ────────────────────────────────────────────────────────────────

function showStatus(msg, type = 'info') {
  const el = document.getElementById('status');
  el.textContent = msg;
  el.className = 'status show ' + type;
}

function hideStatus() {
  document.getElementById('status').className = 'status';
}

function getBotUrl() {
  return document.getElementById('bot-url').value.trim().replace(/\/$/, '');
}

function getToken() {
  return document.getElementById('token').value.trim();
}

function renderDomains() {
  const list = document.getElementById('domains-list');
  list.innerHTML = '';
  for (const d of domains) {
    const chip = document.createElement('div');
    chip.className = 'domain-chip';
    chip.innerHTML = `<span>${d}</span><span class="remove" data-domain="${d}">×</span>`;
    list.appendChild(chip);
  }
  updateSyncButton();
}

function updateSyncButton() {
  const token = getToken();
  document.getElementById('btn-sync').disabled = !token || domains.length === 0;
}

async function updateCurrentSite() {
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab?.url) return;
    const url = new URL(tab.url);
    if (!url.hostname || url.hostname.startsWith('chrome')) return;
    const host = url.hostname.replace(/^www\./, '');
    const el = document.getElementById('current-site');
    el.textContent = host;
    el.dataset.host = host;
    document.getElementById('current-site-row').style.display = 'block';
  } catch {
    // ignore - no active tab or restricted URL
  }
}

// ── Core logic ────────────────────────────────────────────────────────────────

async function syncCookies() {
  const token = getToken();
  if (!token) { showStatus('Paste a token first.', 'error'); return; }

  const botUrl = getBotUrl();
  if (!botUrl) { showStatus('Set the Bot URL first.', 'error'); return; }

  const btn = document.getElementById('btn-sync');
  btn.disabled = true;
  btn.textContent = 'Collecting…';
  hideStatus();

  // Collect cookies for all domains first
  const allCookies = {};
  const skipped = [];

  for (const domain of domains) {
    const cookieList = await chrome.cookies.getAll({ domain });
    if (cookieList.length === 0) {
      skipped.push(domain);
    } else {
      allCookies[domain] = cookieList;
    }
  }

  if (Object.keys(allCookies).length === 0) {
    showStatus('No cookies found. Make sure you\'re logged in on the configured sites.', 'info');
    btn.disabled = false;
    btn.textContent = 'Send Cookies';
    return;
  }

  btn.textContent = 'Sending…';

  // Single request with all domains - token consumed exactly once
  let ok = [], errors = [];
  try {
    const res = await fetch(`${botUrl}/api/v1/dl/cookies/submit`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, cookies: allCookies }),
    });

    if (res.status === 401) {
      showStatus('Token invalid or already used. Get a new one with /cookie token.', 'error');
      document.getElementById('token').value = '';
      updateSyncButton();
      btn.disabled = false;
      btn.textContent = 'Send Cookies';
      return;
    }

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      showStatus(`Server error: ${err.detail || res.status}`, 'error');
      btn.disabled = false;
      btn.textContent = 'Send Cookies';
      return;
    }

    ok = Object.entries(allCookies).map(([d, list]) => `${d} (${list.length})`);
  } catch (e) {
    showStatus(`Network error: ${e}`, 'error');
    btn.disabled = false;
    btn.textContent = 'Send Cookies';
    return;
  }

  // Clear the token - single-use, now consumed
  document.getElementById('token').value = '';
  updateSyncButton();

  const skipNote = skipped.length ? `\nSkipped (no cookies): ${skipped.join(', ')}` : '';
  showStatus(`Sent: ${ok.join(', ')}${skipNote}`, 'success');

  btn.textContent = 'Send Cookies';
  updateSyncButton();
}

// ── Event listeners ───────────────────────────────────────────────────────────

document.getElementById('btn-sync').addEventListener('click', syncCookies);

document.getElementById('bot-url').addEventListener('change', savePersist);

document.getElementById('token').addEventListener('input', updateSyncButton);

document.getElementById('btn-clear-token').addEventListener('click', () => {
  document.getElementById('token').value = '';
  updateSyncButton();
  hideStatus();
});

document.getElementById('btn-add-domain').addEventListener('click', () => {
  const input = document.getElementById('new-domain');
  const val = input.value.trim().toLowerCase()
    .replace(/^www\./, '').replace(/^https?:\/\/[^/]*\/.*/, '')
    .replace(/^https?:\/\//, '');
  if (val && !domains.includes(val)) {
    domains.push(val);
    renderDomains();
    savePersist();
  }
  input.value = '';
});

document.getElementById('new-domain').addEventListener('keydown', e => {
  if (e.key === 'Enter') document.getElementById('btn-add-domain').click();
});

document.getElementById('btn-add-current').addEventListener('click', () => {
  const host = document.getElementById('current-site').dataset.host;
  if (host && !domains.includes(host)) {
    domains.push(host);
    renderDomains();
    savePersist();
  }
});

document.getElementById('domains-list').addEventListener('click', e => {
  const btn = e.target.closest('[data-domain]');
  if (!btn) return;
  domains = domains.filter(x => x !== btn.dataset.domain);
  renderDomains();
  savePersist();
});

// ── Init ──────────────────────────────────────────────────────────────────────

load();
