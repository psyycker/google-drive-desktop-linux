'use strict';

/* gdrive-linux desktop UI. Talks to the Rust side via Tauri commands only. */

const TAURI = window.__TAURI__;
const invoke = (cmd, args) => TAURI.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);

// Line icons (24px grid, stroke = currentColor). Shapes follow the Lucide set (ISC).
const ICONS = {
  cloud: '<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/>',
  cloudCheck: '<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/><path d="m9.5 14 2 2 3.5-3.5"/>',
  checkCircle: '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
  refresh: '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
  loader: '<path d="M21 12a9 9 0 1 1-6.219-8.56"/>',
  pause: '<path d="M9 5v14M15 5v14" stroke-width="2.6"/>',
  play: '<path d="M7 4.5v15l12-7.5z"/>',
  wifiOff: '<path d="M12 20h.01"/><path d="M8.5 16.43a5 5 0 0 1 7 0"/><path d="M5 12.86a10 10 0 0 1 5.17-2.69"/><path d="M19 12.86a10 10 0 0 0-2.01-1.52"/><path d="M2 8.82a15 15 0 0 1 4.18-2.64"/><path d="M22 8.82a15 15 0 0 0-11.29-3.76"/><path d="m2 2 20 20"/>',
  alert: '<circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/>',
  warning: '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
  power: '<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>',
  upload: '<path d="M12 13v8"/><path d="M4 14.9A7 7 0 1 1 15.7 8h1.8a4.5 4.5 0 0 1 2.5 8.2"/><path d="m8 17 4-4 4 4"/>',
  download: '<path d="M12 13v8l-4-4"/><path d="m12 21 4-4"/><path d="M4.4 15.6A7 7 0 1 1 15.7 8h1.8a4.5 4.5 0 0 1 2.5 8.2"/>',
  folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  folderPlus: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/><path d="M12 10v6"/><path d="M9 13h6"/>',
  move: '<path d="M2 9V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H20a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-1"/><path d="M2 13h10"/><path d="m9 16 3-3-3-3"/>',
  trash: '<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>',
  conflict: '<rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  globe: '<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>',
};

const svg = (name, cls = '') =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"${cls ? ` class="${cls}"` : ''}>${ICONS[name] || ''}</svg>`;

const KIND = {
  uploaded: { icon: 'upload', cls: 'up', verb: 'Uploaded' },
  downloaded: { icon: 'download', cls: 'down', verb: 'Downloaded' },
  created_folder_local: { icon: 'folderPlus', cls: 'down', verb: 'Folder created on this computer' },
  created_folder_remote: { icon: 'folderPlus', cls: 'up', verb: 'Folder created in Drive' },
  moved_local: { icon: 'move', cls: '', verb: 'Moved on this computer' },
  moved_remote: { icon: 'move', cls: '', verb: 'Moved in Drive' },
  deleted_local: { icon: 'trash', cls: '', verb: 'Removed from this computer' },
  deleted_remote: { icon: 'trash', cls: '', verb: 'Removed from Drive' },
  conflict: { icon: 'conflict', cls: 'warn', verb: 'Conflict — both versions kept' },
};

const ONBOARDING_STATES = ['setup_required', 'signed_out', 'signing_in'];
const ACTIVE_STATES = ['starting', 'idle', 'syncing', 'paused', 'offline', 'error'];

// ---------------------------------------------------------------- state
let status = null;        // last Status from the daemon, or null when unreachable
let statusError = null;   // error text when unreachable
let config = null;        // last config loaded into the form
let dirty = false;        // form has unsaved edits
let loginUrl = null;      // last URL returned by start_login
let activeTab = 'activity';
let mode = null;          // 'main' | 'onboarding'
let editingStep1 = false;
let polling = false;
const rendered = {};      // last HTML per container, to avoid needless DOM churn

// ---------------------------------------------------------------- helpers
function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function setHtml(el, html) {
  const key = el.id || el;
  if (rendered[key] !== html) {
    rendered[key] = html;
    el.innerHTML = html;
  }
}

function fmtBytes(n) {
  if (n == null) return '';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
  const v = n >= 100 || Math.abs(n - Math.round(n)) < 0.05 ? Math.round(n) : n.toFixed(1);
  return `${v} ${units[i]}`;
}

function rel(iso) {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '';
  const s = (Date.now() - t) / 1000;
  if (s < 45) return 'just now';
  if (s < 90) return '1 min ago';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d === 1) return 'yesterday';
  if (d < 7) return `${d} days ago`;
  return new Date(t).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

const timeTag = (iso) => `<time data-t="${esc(iso)}" title="${esc(new Date(iso).toLocaleString())}">${esc(rel(iso))}</time>`;

function updateTimes() {
  document.querySelectorAll('time[data-t]').forEach((el) => {
    const v = rel(el.dataset.t);
    if (el.textContent !== v) el.textContent = v;
  });
}

const baseName = (p) => p.split('/').filter(Boolean).pop() || p;
function parentOf(p) {
  const parts = p.split('/').filter(Boolean);
  parts.pop();
  return parts.join('/');
}
function joinRoot(rel) {
  const root = (status && status.sync_root) || (config && config.sync_root) || '';
  if (!root) return null;
  return rel ? `${root.replace(/\/+$/, '')}/${rel}` : root;
}
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

let toastTimer = null;
function toast(msg, isError = false) {
  const el = $('toast');
  el.textContent = msg;
  el.classList.toggle('error', isError);
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, isError ? 6000 : 3000);
}

async function run(cmd, args, okMsg) {
  try {
    const r = await invoke(cmd, args);
    if (okMsg) toast(okMsg);
    return r;
  } catch (e) {
    toast(String(e), true);
    throw e;
  }
}

// Busy-state wrapper for buttons.
async function withBusy(btn, fn) {
  if (btn.disabled) return;
  btn.disabled = true;
  try { await fn(); } catch (_) { /* already reported */ } finally { btn.disabled = false; }
}

// ---------------------------------------------------------------- polling
async function poll() {
  if (polling) return;
  polling = true;
  try {
    status = await invoke('get_status');
    statusError = null;
  } catch (e) {
    status = null;
    statusError = String(e);
  } finally {
    polling = false;
  }
  render();
}

// ---------------------------------------------------------------- rendering
function render() {
  const onboarding = !!status && ONBOARDING_STATES.includes(status.state);
  const newMode = onboarding ? 'onboarding' : 'main';
  if (newMode !== mode) {
    mode = newMode;
    placeSharedBlocks(onboarding);
    $('main-view').hidden = onboarding;
    $('onboarding').hidden = !onboarding;
    $('banner').hidden = onboarding;
    if (!dirty) loadConfig();
  }
  if (status && status.account) loginUrl = null;

  renderHeader();
  if (onboarding) {
    renderOnboarding();
  } else {
    renderBanner();
    renderTransfers();
    renderRecent();
    renderErrors();
    renderAccountCard();
  }
  renderLoginUrls();
  updateSavebar();
  updateTimes();
}

function hueFor(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

function renderHeader() {
  const acct = status && status.account;
  const avatar = $('avatar');
  if (acct) {
    const label = acct.display_name || acct.email;
    const initial = (label.trim()[0] || '?').toUpperCase();
    setHtml(avatar, esc(initial));
    avatar.style.background = `hsl(${hueFor(acct.email)} 55% 48%)`;
    $('acct-name').textContent = acct.display_name || acct.email;
    $('acct-email').textContent = acct.display_name ? acct.email : 'Google Drive';
  } else {
    setHtml(avatar, svg('cloud'));
    avatar.style.background = '';
    $('acct-name').textContent = 'Google Drive';
    $('acct-email').textContent = status ? 'Not signed in' : 'Sync service not running';
  }

  const q = status && status.quota;
  $('quota').hidden = !q || !acct;
  if (q) {
    const fill = $('quota-fill');
    if (q.limit) {
      const pct = Math.min(100, (q.used / q.limit) * 100);
      fill.style.width = `${pct.toFixed(1)}%`;
      fill.classList.toggle('warn', pct >= 90 && pct < 100);
      fill.classList.toggle('full', pct >= 100);
      $('quota-text').textContent = `${fmtBytes(q.used)} of ${fmtBytes(q.limit)} used`;
    } else {
      fill.style.width = '0';
      $('quota-text').textContent = `${fmtBytes(q.used)} used · unlimited storage`;
    }
  }
  $('btn-open-folder').disabled = !joinRoot('');
}

function bannerInfo() {
  if (!status) {
    return {
      tone: 'error', icon: 'power', title: 'Sync service not running',
      sub: 'The gdrived service could not be started. This window reconnects automatically once it runs.',
    };
  }
  const s = status;
  const n = s.transfers.length;
  switch (s.state) {
    case 'starting':
      return { tone: 'busy', icon: 'loader', spin: true, title: 'Starting…', sub: 'Connecting to Google Drive' };
    case 'idle': {
      const errs = s.errors.length ? `${plural(s.errors.length, 'item', 'items')} couldn’t sync · ` : '';
      const last = s.last_synced ? `Last synced ${timeTag(s.last_synced)}` : 'Everything is in sync';
      return { tone: s.errors.length ? 'warn' : 'ok', icon: s.errors.length ? 'warning' : 'cloudCheck', title: 'Up to date', sub: errs + last, html: true };
    }
    case 'syncing': {
      const title = n > 0 ? `Syncing ${plural(n, 'item', 'items')}` : s.pending > 0 ? `Syncing ${plural(s.pending, 'item', 'items')}` : 'Syncing…';
      const sub = s.pending > n ? `${plural(s.pending, 'change', 'changes')} waiting` : 'Keeping your files up to date';
      return { tone: 'busy', icon: 'refresh', spin: true, title, sub };
    }
    case 'paused':
      return { tone: 'neutral', icon: 'pause', title: 'Paused', sub: s.pending ? `${plural(s.pending, 'change', 'changes')} will sync when you resume` : 'Changes will sync when you resume' };
    case 'offline':
      return { tone: 'warn', icon: 'wifiOff', title: 'Offline — retrying', sub: s.message || 'Waiting for a network connection' };
    case 'error':
      return { tone: 'error', icon: 'alert', title: 'Sync error', sub: s.message || 'Syncing stopped because of an error' };
    default:
      return { tone: 'neutral', icon: 'cloud', title: s.message || s.state, sub: '' };
  }
}

function renderBanner() {
  const b = bannerInfo();
  const banner = $('banner');
  banner.dataset.tone = b.tone;
  setHtml($('banner-icon'), svg(b.icon, b.spin ? 'spin' : ''));
  $('banner-title').textContent = b.title;
  setHtml($('banner-sub'), b.html ? b.sub : esc(b.sub));
  $('banner-sub').title = b.html ? '' : b.sub;

  const active = !!status && ACTIVE_STATES.includes(status.state);
  const paused = !!status && status.state === 'paused';
  const pauseBtn = $('btn-pause');
  pauseBtn.hidden = !active;
  setHtml(pauseBtn, paused ? 'Resume' : 'Pause');
  pauseBtn.dataset.action = paused ? 'resume' : 'pause';
  $('btn-sync').hidden = !active || paused;
}

function renderTransfers() {
  const box = $('transfers');
  const list = status ? status.transfers : [];
  if (!list.length) {
    setHtml(box, '');
    return;
  }
  // Keyed update so progress bars animate smoothly instead of being recreated.
  if (!box.querySelector('.section-label')) {
    box.innerHTML = '<div class="section-label">In progress</div><div class="list" id="transfer-list"></div>';
    rendered[box.id] = null;
  }
  const holder = $('transfer-list');
  const seen = new Set();
  for (const t of list) {
    const key = `${t.direction}:${t.path}`;
    seen.add(key);
    let el = holder.querySelector(`[data-key="${CSS.escape(key)}"]`);
    if (!el) {
      el = document.createElement('div');
      el.className = 'item transfer';
      el.dataset.key = key;
      el.dataset.path = t.path;
      const up = t.direction === 'upload';
      el.innerHTML = `
        <div class="item-icon ${up ? 'up' : 'down'}">${svg(up ? 'upload' : 'download')}</div>
        <div class="item-main">
          <div class="item-name" title="${esc(t.path)}">${esc(baseName(t.path))}</div>
          <div class="progress ${up ? '' : 'down'}"><div></div></div>
          <div class="item-sub"></div>
        </div>`;
      holder.appendChild(el);
    }
    const pct = t.bytes_total > 0 ? Math.min(100, (t.bytes_done / t.bytes_total) * 100) : 0;
    const bar = el.querySelector('.progress');
    bar.classList.toggle('indeterminate', !(t.bytes_total > 0));
    bar.firstElementChild.style.width = `${pct.toFixed(1)}%`;
    const verb = t.direction === 'upload' ? 'Uploading' : 'Downloading';
    const sub = t.bytes_total > 0 ? `${verb} · ${fmtBytes(t.bytes_done)} of ${fmtBytes(t.bytes_total)}` : `${verb}…`;
    const subEl = el.querySelector('.item-sub');
    if (subEl.textContent !== sub) subEl.textContent = sub;
  }
  holder.querySelectorAll('[data-key]').forEach((el) => { if (!seen.has(el.dataset.key)) el.remove(); });
}

function locationLabel(path) {
  const parent = parentOf(path);
  return parent ? `in ${parent}` : 'in My Drive';
}

function renderRecent() {
  const recent = status ? status.recent : [];
  $('recent-empty').hidden = !status || recent.length > 0 || status.transfers.length > 0;
  if (!recent.length) {
    setHtml($('recent'), '');
    return;
  }
  const rows = recent.map((a) => {
    const k = KIND[a.kind] || { icon: 'cloud', cls: '', verb: a.kind };
    const detail = a.detail ? ` · ${esc(a.detail)}` : '';
    return `
      <div class="item" data-open="${esc(a.path)}" title="${esc(a.detail ? `${a.path}\n${a.detail}` : a.path)}">
        <div class="item-icon ${k.cls}">${svg(k.icon)}</div>
        <div class="item-main">
          <div class="item-name">${esc(baseName(a.path))}</div>
          <div class="item-sub"><span class="verb">${esc(k.verb)}</span> · ${esc(locationLabel(a.path))}${detail}</div>
        </div>
        <div class="item-time">${timeTag(a.time)}</div>
      </div>`;
  });
  setHtml($('recent'), `<div class="section-label">Recent activity</div>${rows.join('')}`);
}

function renderErrors() {
  const errors = status ? status.errors : [];
  const badge = $('errors-badge');
  badge.hidden = errors.length === 0;
  badge.textContent = errors.length > 99 ? '99+' : String(errors.length);
  $('errors-empty').hidden = !status || errors.length > 0;
  const rows = errors.map((e) => `
    <div class="item" data-open="${esc(e.path)}" title="${esc(e.path)}">
      <div class="item-icon danger">${svg('warning')}</div>
      <div class="item-main">
        <div class="item-name">${esc(baseName(e.path) || 'Google Drive')}</div>
        <div class="item-sub">${esc(locationLabel(e.path))}</div>
        <div class="error-msg">${esc(e.message)}</div>
      </div>
      <div class="item-time">${timeTag(e.time)}</div>
    </div>`);
  setHtml($('errors'), rows.join(''));
}

function renderAccountCard() {
  const acct = status && status.account;
  setHtml($('account-line'), acct
    ? `Signed in as <b class="selectable">${esc(acct.email)}</b>`
    : status ? 'Not signed in' : 'Sync service not running');
  $('btn-signin').hidden = !!acct;
  $('btn-signin').disabled = !status || !config || !(config.client_id && config.client_secret);
  $('btn-signout').hidden = !acct;
  if (!acct) $('signout-confirm').hidden = true;
  $('btn-resync').disabled = !acct;
}

function renderOnboarding() {
  const state = status.state;
  const step1 = $('step-1');
  const step2 = $('step-2');
  const needCreds = state === 'setup_required';
  const showStep1Body = needCreds || editingStep1;

  step1.classList.toggle('done', !needCreds);
  $('step-1-body').hidden = !showStep1Body;
  $('step-1-done').hidden = showStep1Body;
  $('btn-step1-edit').hidden = needCreds || editingStep1;

  step2.classList.toggle('disabled', needCreds);
  const hint = $('step-2-hint');
  const signBtn = $('btn-onb-signin');
  if (state === 'signing_in') {
    hint.textContent = 'Finish signing in in your browser. This window updates automatically.';
    hint.style.color = '';
    signBtn.textContent = 'Restart sign-in';
  } else {
    const msg = state === 'signed_out' && status.message;
    hint.textContent = msg || 'Your browser will open so you can allow access to Google Drive.';
    hint.style.color = msg ? 'var(--danger)' : '';
    signBtn.textContent = 'Sign in with Google';
  }
}

function loginUrlHtml() {
  return `<div>If your browser didn’t open, copy this link into it:</div>
    <div class="url-row"><input type="text" readonly value="${esc(loginUrl)}"><button class="btn btn-sm" data-copy>Copy</button></div>`;
}

function renderLoginUrls() {
  const show = !!loginUrl && !!status && ['signing_in', 'signed_out'].includes(status.state);
  for (const id of ['login-url-onb', 'login-url-settings']) {
    const el = $(id);
    el.hidden = !show;
    setHtml(el, show ? loginUrlHtml() : '');
  }
}

// The credential and folder fields live in Settings, and are moved into the onboarding
// steps while onboarding is shown, so there is one set of inputs.
function placeSharedBlocks(onboarding) {
  if (onboarding) {
    $('cred-slot').appendChild($('cred-block'));
    $('folder-slot').appendChild($('folder-block'));
  } else {
    $('cred-home').appendChild($('cred-block'));
    $('folder-home').appendChild($('folder-block'));
  }
}

// ---------------------------------------------------------------- settings
function fillForm(c) {
  $('f-client-id').value = c.client_id || '';
  $('f-client-secret').value = c.client_secret || '';
  $('f-root').value = c.sync_root || '';
  $('f-poll').value = c.poll_interval_secs ?? 15;
  $('f-conc').value = c.max_concurrent_transfers ?? 4;
  $('f-trash').checked = !!c.use_local_trash;
  $('f-ignore').value = (c.ignore || []).join('\n');
}

async function loadConfig() {
  try {
    config = await invoke('get_config');
    fillForm(config);
    setDirty(false);
  } catch (e) {
    toast(`Couldn’t load settings: ${e}`, true);
  }
}

function collectConfig() {
  const int = (id, min, max, name) => {
    const v = Number.parseInt($(id).value, 10);
    if (!Number.isFinite(v) || v < min || v > max) throw new Error(`${name} must be between ${min} and ${max}`);
    return v;
  };
  const root = $('f-root').value.trim();
  if (!root.startsWith('/')) throw new Error('The Google Drive folder must be an absolute path');
  return {
    ...(config || {}),
    client_id: $('f-client-id').value.trim(),
    client_secret: $('f-client-secret').value.trim(),
    sync_root: root.length > 1 ? root.replace(/\/+$/, '') : root,
    poll_interval_secs: int('f-poll', 5, 3600, 'Check interval'),
    max_concurrent_transfers: int('f-conc', 1, 16, 'Parallel transfers'),
    use_local_trash: $('f-trash').checked,
    ignore: $('f-ignore').value.split('\n').map((l) => l.trim()).filter(Boolean),
  };
}

async function saveConfig(okMsg = 'Settings saved') {
  let next;
  try {
    next = collectConfig();
  } catch (e) {
    toast(e.message, true);
    throw e;
  }
  await run('set_config', { config: next }, okMsg);
  config = next;
  setDirty(false);
  await loadConfig();
  poll();
}

function setDirty(v) {
  dirty = v;
  updateSavebar();
}

function updateSavebar() {
  $('savebar').hidden = !(dirty && mode === 'main' && activeTab === 'settings');
}

async function browseFolder() {
  const current = $('f-root').value.trim() || undefined;
  const opts = { directory: true, multiple: false, defaultPath: current, title: 'Choose your Google Drive folder' };
  try {
    const picked = TAURI.dialog && TAURI.dialog.open
      ? await TAURI.dialog.open(opts)
      : await invoke('plugin:dialog|open', { options: opts });
    if (typeof picked === 'string' && picked) {
      $('f-root').value = picked;
      setDirty(true);
    }
  } catch (e) {
    toast(`Couldn’t open the folder picker: ${e}`, true);
  }
}

// ---------------------------------------------------------------- actions
async function startLogin(btn) {
  await withBusy(btn, async () => {
    if (dirty) await saveConfig('Settings saved');
    loginUrl = await run('start_login');
    render();
    poll();
  });
}

function selectTab(name) {
  activeTab = name;
  document.querySelectorAll('.tab').forEach((t) => t.classList.toggle('active', t.dataset.tab === name));
  for (const p of ['activity', 'errors', 'settings']) $(`tab-${p}`).hidden = p !== name;
  if (name === 'settings' && !dirty) {
    loadConfig();
    loadAutostart();
  }
  updateSavebar();
}

async function loadAutostart() {
  try { $('f-autostart').checked = await invoke('get_autostart'); } catch (_) { /* ignore */ }
}

function copyFrom(btn) {
  const input = btn.parentElement.querySelector('input');
  const done = () => {
    btn.textContent = 'Copied';
    setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
  };
  const fallback = () => {
    input.select();
    try { document.execCommand('copy'); done(); } catch (_) { toast('Select the link and copy it manually', true); }
  };
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(input.value).then(done, fallback);
  } else {
    fallback();
  }
}

function bind() {
  document.querySelectorAll('[data-icon]').forEach((el) => { el.innerHTML = svg(el.dataset.icon); });
  $('btn-open-folder').innerHTML = svg('folder');
  $('btn-open-web').innerHTML = svg('globe');

  $('btn-open-folder').addEventListener('click', () => {
    const root = joinRoot('');
    if (root) run('open_path', { path: root });
  });
  $('btn-open-web').addEventListener('click', () => run('open_url', { url: 'https://drive.google.com' }));

  const onClick = (id, fn) => {
    const btn = $(id);
    btn.addEventListener('click', () => fn(btn));
  };

  onClick('btn-pause', (btn) => withBusy(btn, async () => {
    await run(btn.dataset.action);
    await poll();
  }));
  onClick('btn-sync', (btn) => withBusy(btn, async () => {
    await run('sync_now', undefined, 'Checking for changes…');
    await poll();
  }));

  document.querySelectorAll('.tab').forEach((t) => t.addEventListener('click', () => selectTab(t.dataset.tab)));

  // Click an activity/error row to open the folder that contains it.
  for (const id of ['recent', 'errors', 'transfers']) {
    $(id).addEventListener('click', (e) => {
      const row = e.target.closest('[data-open], [data-path]');
      if (!row) return;
      const rel = row.dataset.open ?? row.dataset.path;
      const dir = joinRoot(parentOf(rel));
      if (dir) run('open_path', { path: dir });
    });
  }

  // Settings form.
  document.querySelectorAll('[data-cfg]').forEach((el) => {
    el.addEventListener('input', () => setDirty(true));
    el.addEventListener('change', () => setDirty(true));
  });
  $('btn-browse').addEventListener('click', browseFolder);
  onClick('btn-reveal', (btn) => {
    const input = $('f-client-secret');
    const show = input.type === 'password';
    input.type = show ? 'text' : 'password';
    btn.textContent = show ? 'Hide' : 'Show';
  });
  onClick('btn-save', (btn) => withBusy(btn, () => saveConfig()));
  $('btn-discard').addEventListener('click', () => { if (config) fillForm(config); setDirty(false); });
  const autostart = $('f-autostart');
  autostart.addEventListener('change', async () => {
    const on = autostart.checked;
    try {
      await run('set_autostart', { enabled: on }, on ? 'Google Drive will start when you log in' : 'Start on login turned off');
    } catch (_) {
      autostart.checked = !on;
    }
  });

  // Account.
  onClick('btn-signin', startLogin);
  $('btn-signout').addEventListener('click', () => { $('signout-confirm').hidden = false; });
  $('btn-signout-cancel').addEventListener('click', () => { $('signout-confirm').hidden = true; });
  onClick('btn-signout-confirm', (btn) => withBusy(btn, async () => {
    await run('sign_out', undefined, 'Signed out');
    $('signout-confirm').hidden = true;
    await poll();
  }));
  onClick('btn-resync', (btn) => withBusy(btn, async () => {
    await run('full_resync', undefined, 'Full resync started');
    await poll();
  }));

  // Onboarding.
  onClick('btn-step1-save', (btn) => withBusy(btn, async () => {
    if (!$('f-client-id').value.trim() || !$('f-client-secret').value.trim()) {
      toast('Enter both the client ID and the client secret', true);
      return;
    }
    await saveConfig('Saved');
    editingStep1 = false;
    render();
  }));
  $('btn-step1-edit').addEventListener('click', () => { editingStep1 = true; render(); });
  onClick('btn-onb-signin', startLogin);

  // Copy buttons in login URL boxes (rendered dynamically).
  document.addEventListener('click', (e) => {
    const copy = e.target.closest('[data-copy]');
    if (copy) copyFrom(copy);
  });

  // Never navigate the webview: send links to the default browser.
  document.addEventListener('click', (e) => {
    const a = e.target.closest('a[href]');
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute('href');
    if (/^https?:\/\//.test(href)) run('open_url', { url: href });
  });
}

async function init() {
  bind();
  await loadConfig();
  loadAutostart();
  await poll();

  // Poll once a second while visible; the tray keeps its own (slower) poll.
  setInterval(() => { if (!document.hidden) poll(); }, 1000);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) poll(); });
  if (TAURI.event && TAURI.event.listen) {
    TAURI.event.listen('app://shown', () => {
      poll();
      if (!dirty) loadConfig();
    });
  }
}

init();
