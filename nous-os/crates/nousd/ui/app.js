/* NOUS Shell.
 *
 * No framework, no build step, no dependency — the file you are reading is the
 * file that runs. The shell is a thin client over the daemon's capability API:
 * every list, every play, every tidy goes through `cap.invoke` and is
 * adjudicated by the same policy engine that governs the command shell. There
 * is no privileged path for the desktop, which is the point.
 */
'use strict';

const $  = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

/* Everything that reaches the DOM goes through here. Filenames are attacker-
 * controlled in the only sense that matters: you did not write them. */
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) =>
  ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

const state = {
  cwd: null,
  home: null,
  plan: null,
  status: null,
  playing: null,
  /// Whether this window is the summon overlay rather than the full shell.
  overlay: false,
  /// What the user was looking at when they summoned NOUS.
  context: {},
};

// ---------------------------------------------------------------- transport

let rpcId = 0;

async function rpc(method, params = {}) {
  const res = await fetch('/api', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id: String(++rpcId), method, params }),
  });
  const body = await res.json();
  if (body.ok) return body.result;
  throw new Error(body.error ? body.error.message : 'the daemon did not answer');
}

/* Exercise one capability. The shell never reaches past this. */
const invoke = (capability, args = {}, extra = {}) =>
  rpc('cap.invoke', { capability, args, ...extra });

/* Pull the value out of a single-step run, or throw with the daemon's reason. */
function firstValue(run) {
  const step = (run.results || [])[0];
  if (!step) throw new Error(run.message || 'nothing ran');
  if (step.state !== 'ok') throw new Error(step.detail || run.message || step.state);
  return step.value || {};
}

// ------------------------------------------------------------------- toasts

function toast(message, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.innerHTML = `<i class="mark-dot"></i><div>${esc(message)}</div>`;
  $('#toasts').append(el);
  setTimeout(() => {
    el.style.transition = 'opacity 240ms, transform 240ms';
    el.style.opacity = '0';
    el.style.transform = 'translateX(18px)';
    setTimeout(() => el.remove(), 260);
  }, 5200);
}

// -------------------------------------------------------------- formatting

function bytes(n) {
  if (!n) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${i === 0 ? v : v.toFixed(1)} ${u[i]}`;
}

function ago(unixSeconds) {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

const ICONS = {
  folder: '📁', audio: '♪', video: '▶', image: '▣', document: '▤', sheet: '▦',
  slides: '▥', text: '≡', code: '⟨⟩', config: '⚙', archive: '▢', package: '◈',
  'image-disk': '◉', file: '·',
};
const icon = (kind) => ICONS[kind] || ICONS.file;

// ---------------------------------------------------------------- the plan

/* The plan card. Nothing runs before this has been drawn and answered — the
 * shell has no path that skips it. */
function renderPlan(preflight) {
  state.plan = preflight.plan;
  const host = $('#plan-host');
  const steps = preflight.steps || [];

  if (preflight.clarification) {
    host.innerHTML = `<div class="plan"><div class="card-body">
      <div class="muted">${esc(preflight.clarification)}</div></div></div>`;
    return;
  }
  if (!steps.length) {
    host.innerHTML = '';
    return;
  }

  const originIsModel = (preflight.origin || '').startsWith('model');
  const rows = steps.map((s) => `
    <div class="step" data-risk="${esc(s.risk)}" data-id="${esc(s.id)}">
      <i class="rail-dot"></i>
      <div class="body">
        <div class="what">${esc(s.summary)}</div>
        <div class="cap">${esc(s.capability)}</div>
      </div>
      <span class="verdict ${esc(s.decision)}">${esc(s.decision)}</span>
    </div>`).join('');

  const blocked = preflight.blocked;
  const needs = preflight.needs_approval;

  host.innerHTML = `
    <div class="plan">
      <div class="plan-head">
        <span class="title">${esc(preflight.utterance)}</span>
        <span class="grow"></span>
        <span class="origin ${originIsModel ? 'model' : ''}">
          <i class="dot"></i>${originIsModel ? esc(preflight.origin) : 'understood locally'}
        </span>
      </div>
      ${rows}
      <div class="plan-foot">
        <span class="faint">${
          blocked ? 'Policy refuses part of this.'
          : needs ? 'This changes things. Nothing has happened yet.'
          : 'Read-only — safe to run.'
        }</span>
        <span class="grow"></span>
        <button class="btn ghost small" id="plan-cancel">Dismiss</button>
        <button class="btn small" id="plan-dry">Preview</button>
        <button class="btn primary small" id="plan-run" ${blocked ? 'disabled' : ''}>
          ${needs ? 'Approve and run' : 'Run'}
        </button>
      </div>
    </div>`;

  $('#plan-cancel').onclick = () => { host.innerHTML = ''; state.plan = null; };
  $('#plan-dry').onclick = () => runPlan({ dry_run: true, approved: true });
  $('#plan-run').onclick = () => runPlan({ approved: true });
}

async function runPlan(opts) {
  if (!state.plan) return;
  const host = $('#plan-host');
  $$('.step', host).forEach((el) => el.classList.add('running'));
  try {
    const run = await rpc('intent.submit', {
      text: state.plan.utterance,
      plan: state.plan,
      context: state.context,
      ...opts,
    });
    renderRun(run, opts.dry_run);
    refreshLedger();
    if (currentView() === 'home') refreshHome();
  } catch (e) {
    toast(e.message, 'bad');
    $$('.step', host).forEach((el) => el.classList.remove('running'));
  }
}

function renderRun(run, wasDryRun) {
  const host = $('#plan-host');
  (run.results || []).forEach((r) => {
    const el = $(`.step[data-id="${CSS.escape(r.id)}"]`, host);
    if (!el) return;
    el.classList.remove('running');
    el.classList.add('done');
    const verdict = $('.verdict', el);
    if (verdict) {
      verdict.textContent = r.state === 'ok' ? (wasDryRun ? 'preview' : 'done') : r.state;
      verdict.className = `verdict ${r.state === 'ok' ? 'allow' : 'deny'}`;
    }
    if (r.detail) $('.cap', el).textContent = r.detail;
  });

  const foot = $('.plan-foot', host);
  if (foot) {
    const message = run.message || (wasDryRun ? 'Preview only — nothing was changed.' : 'Done.');
    foot.innerHTML = `<span class="faint">${esc(message)}</span><span class="grow"></span>
      <button class="btn ghost small" id="plan-cancel">Close</button>`;
    $('#plan-cancel').onclick = () => { host.innerHTML = ''; state.plan = null; };
  }

  if (run.status === 'blocked') toast(run.message, 'bad');
  else if (run.status === 'stopped') toast(run.message || 'Nothing to do.', 'warn');

  // A proposal is the one result the shell renders specially: it is the
  // curator's suggestion, and it deserves its own decision.
  const proposal = (run.results || []).find((r) => r.value && Array.isArray(r.value.steps));
  if (proposal) showProposal(proposal.value);
}

function showProposal(value) {
  const steps = value.steps || [];
  if (!steps.length) return;
  const host = $('#plan-host');
  const list = steps.slice(0, 12).map((s) => `
    <div class="step" data-risk="write">
      <i class="rail-dot"></i>
      <div class="body"><div class="what">${esc(s.summary)}</div></div>
    </div>`).join('');
  const more = steps.length > 12 ? `<div class="step"><div class="body faint">…and ${steps.length - 12} more</div></div>` : '';

  host.insertAdjacentHTML('beforeend', `
    <div class="plan" id="proposal">
      <div class="plan-head">
        <span class="title">Proposed tidy-up</span>
        <span class="grow"></span>
        <span class="faint">${esc(steps.length)} moves · ${esc(value.summary || '')}</span>
      </div>
      ${list}${more}
      <div class="plan-foot">
        <span class="faint">Nothing is deleted. Everything here can be undone from the Ledger.</span>
        <span class="grow"></span>
        <button class="btn ghost small" id="prop-no">Not now</button>
        <button class="btn primary small" id="prop-yes">Apply</button>
      </div>
    </div>`);

  $('#prop-no').onclick = () => $('#proposal').remove();
  $('#prop-yes').onclick = async () => {
    try {
      const run = await invoke('curate.apply', { steps }, { approved: true, why: 'apply tidy-up' });
      const v = firstValue(run);
      toast(`Tidied ${v.applied} item${v.applied === 1 ? '' : 's'}.`);
      $('#proposal').remove();
      refreshLedger();
      if (currentView() === 'files') openDir(state.cwd);
    } catch (e) {
      toast(e.message, 'bad');
    }
  };
}

/* The chips that show what NOUS already knows. They are worth the space: the
 * difference between "delete these" working and not is whether the user can see
 * that the system knows what "these" are. */
function renderContext() {
  const strip = $('#context-strip');
  const chips = [];
  const { focus, paths, cwd } = state.context;

  if (Array.isArray(paths) && paths.length) {
    const names = paths.map((p) => p.split('/').pop());
    const label = paths.length === 1
      ? names[0]
      : `${paths.length} items`;
    chips.push(`<span class="chip" title="${esc(paths.join('\n'))}">
      <i class="ico">◈</i><span class="trunc">selected <b>${esc(label)}</b></span></span>`);
  }
  if (cwd) {
    chips.push(`<span class="chip"><i class="ico">▸</i><span class="trunc">in <b>${esc(cwd.split('/').pop() || cwd)}</b></span></span>`);
  }
  if (focus) {
    chips.push(`<span class="chip"><i class="ico">□</i><span class="trunc">${esc(focus)}</span></span>`);
  }

  if (!chips.length) { strip.hidden = true; return; }
  strip.innerHTML = chips.join('');
  strip.hidden = false;
}

// ---------------------------------------------------------------- the views

const currentView = () => $('.view.active').id.replace('view-', '');

function showView(name) {
  $$('.view').forEach((v) => v.classList.toggle('active', v.id === `view-${name}`));
  $$('.ctx').forEach((c) => c.setAttribute('aria-selected', String(c.dataset.view === name)));
  ({ files: () => openDir(state.cwd || state.home),
     media: refreshMedia,
     ledger: refreshLedger,
     system: refreshSystem,
     home: refreshHome }[name] || (() => {}))();
}

// --- home ------------------------------------------------------------------

async function refreshHome() {
  const s = state.status;
  if (!s) return;
  const m = s.metrics || {};
  const disk = m.disk_used_pct || 0;
  const mem = m.mem_used_pct || 0;
  const load = m.load1 || 0;
  const cpus = m.cpus || 1;

  $('#home-sub').textContent = `${s.system.distro || 'this machine'} · up ${Math.floor((s.uptime_secs || 0) / 60)}m`;
  const meters = [
    ['Disk',       `${disk.toFixed(0)}<small>%</small>`,          disk,                              disk > 90],
    ['Memory',     `${mem.toFixed(0)}<small>%</small>`,           mem,                               mem > 88],
    ['Load',       load.toFixed(2),                               Math.min(100, (load / cpus) * 100), load / cpus > 2],
    ['Free space', bytes((m.disk_free_kb || 0) * 1024),           100 - disk,                        disk > 90],
  ];
  $('#home-meters').innerHTML = meters.map(([k, v, , warn]) => meter(k, v, warn)).join('');
  // Widths go through the CSSOM: a `style` attribute would be refused by the
  // page's Content-Security-Policy, and silently render every bar full.
  $$('#home-meters .bar i').forEach((el, i) => {
    el.style.width = `${Math.max(2, Math.min(100, meters[i][2]))}%`;
  });
}

const meter = (k, v, warn) => `
  <div class="meter">
    <div class="k">${esc(k)}</div>
    <div class="v">${v}</div>
    <div class="bar ${warn ? 'warn' : ''}"><i></i></div>
  </div>`;

async function scanNow() {
  const host = $('#home-findings');
  host.innerHTML = '<div class="empty-state">Looking…</div>';
  try {
    const value = firstValue(await invoke('curate.scan', {}, { why: 'look for things to tidy' }));
    const findings = value.findings || [];
    if (!findings.length) {
      host.innerHTML = '<div class="empty-state"><div class="big">✓</div>Nothing needs tidying.</div>';
      return;
    }
    host.innerHTML = `<div class="rows">${findings.map((f) => `
      <div class="row">
        <span class="ico">${f.severity >= 3 ? '●' : '○'}</span>
        <span class="name">${esc(f.title)}<div class="faint fs-12">${esc(f.detail)}</div></span>
        <span class="meta">${esc(bytes(f.bytes))}</span>
      </div>`).join('')}</div>
      <div class="mt-14"><button class="btn small" id="btn-propose">Show me what you'd do</button></div>`;
    $('#btn-propose').onclick = async () => {
      const v = firstValue(await invoke('curate.propose', {}, { why: 'plan a tidy-up' }));
      showProposal(v);
      $('#plan-host').scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    };
  } catch (e) {
    host.innerHTML = `<div class="empty-state">${esc(e.message)}</div>`;
  }
}

// --- files -----------------------------------------------------------------

async function openDir(path) {
  if (!path) return;
  try {
    const value = firstValue(await invoke(`fs.list:${path}`, { path }, { why: `list ${path}` }));
    state.cwd = value.path;
    renderCrumbs(value.path);
    const entries = value.entries || [];
    $('#file-grid').innerHTML = entries.length
      ? entries.map((e) => `
        <button class="tile" data-path="${esc(e.path)}" data-dir="${e.is_dir}" data-kind="${esc(e.kind)}">
          <span class="ico">${icon(e.kind)}</span>
          <div class="name">${esc(e.name)}</div>
          <div class="meta">${e.is_dir ? 'folder' : bytes(e.size)}</div>
        </button>`).join('')
      : '<div class="empty-state"><div class="big">◌</div>This folder is empty.</div>';

    $$('#file-grid .tile').forEach((t) => {
      t.onclick = () => {
        if (t.dataset.dir === 'true') openDir(t.dataset.path);
        else if (['audio', 'video'].includes(t.dataset.kind)) play(t.dataset.path);
        else toast(t.dataset.path);
      };
    });
  } catch (e) {
    toast(e.message, 'bad');
  }
}

function renderCrumbs(path) {
  const parts = path.split('/').filter(Boolean);
  let acc = '';
  const crumbs = [{ label: '/', path: '/' }];
  for (const p of parts) { acc += `/${p}`; crumbs.push({ label: p, path: acc }); }
  $('#crumbs').innerHTML = crumbs
    .map((c, i) => `${i ? '<span class="sep">/</span>' : ''}<button data-path="${esc(c.path)}">${esc(c.label)}</button>`)
    .join('');
  $$('#crumbs button').forEach((b) => { b.onclick = () => openDir(b.dataset.path); });
}

// --- media -----------------------------------------------------------------

async function refreshMedia() {
  try {
    const value = firstValue(await invoke('media.search', { query: '', limit: 60 }, { why: 'browse the library' }));
    const items = value.items || [];
    $('#media-sub').textContent = `${value.total || 0} in the library`;
    $('#media-rows').innerHTML = items.length
      ? items.map((i) => `
        <button class="row" data-path="${esc(i.path)}">
          <span class="ico">${icon(i.kind)}</span>
          <span class="name">${esc(i.title || i.name)}</span>
          <span class="meta">${esc(bytes(i.size))}</span>
        </button>`).join('')
      : `<div class="empty-state"><div class="big">♪</div>The library is empty.
           <div class="mt-12"><button class="btn small" id="btn-scanlib">Scan for media</button></div></div>`;
    $$('#media-rows .row').forEach((r) => { r.onclick = () => play(r.dataset.path); });
    const scanBtn = $('#btn-scanlib');
    if (scanBtn) scanBtn.onclick = reindexMedia;
  } catch (e) {
    $('#media-rows').innerHTML = `<div class="empty-state">${esc(e.message)}</div>`;
  }
}

async function reindexMedia() {
  toast('Scanning for media…');
  try {
    const v = firstValue(await invoke('media.index', { probe: false }, { approved: true, why: 'index media' }));
    toast(`Found ${v.count} media files.`);
    refreshMedia();
  } catch (e) { toast(e.message, 'bad'); }
}

async function play(path) {
  try {
    await invoke('media.play', { path }, { approved: true, why: `play ${path}` });
    state.playing = path;
    $('#now-title').textContent = path.split('/').pop();
    $('#now-sub').textContent = 'playing';
  } catch (e) { toast(e.message, 'warn'); }
}

async function mediaControl(action) {
  try {
    await invoke('media.control', { action }, { approved: true, why: action });
  } catch (e) { toast(e.message, 'warn'); }
}

// --- ledger ----------------------------------------------------------------

async function refreshLedger() {
  try {
    const out = await rpc('journal.tail', { limit: 60 });
    const records = (out.records || []).reverse();
    $('#ledger').innerHTML = records.length
      ? records.map((r) => {
          const undoable = r.undo && r.undo.kind && !r.undone_by &&
                           ['executed', 'confirmed'].includes(r.outcome);
          const tag = r.outcome === 'refused' ? '<span class="tag refused">refused</span>'
                    : r.outcome === 'dry-run' ? '<span class="tag dry">preview</span>' : '';
          return `<div class="entry ${r.undone_by ? 'undone' : ''}" data-risk="${esc(r.risk)}" data-outcome="${esc(r.outcome)}">
            <div class="line">
              <span class="detail">${esc(r.detail || r.capability)}</span>
              ${tag}
              <span class="when">${esc(ago(r.ts))}</span>
              ${undoable ? `<button class="btn small" data-undo="${r.seq}">Undo</button>` : ''}
            </div>
            <div class="cap">${esc(r.capability)} · ${esc(r.intent)}</div>
          </div>`;
        }).join('')
      : '<div class="empty-state"><div class="big">◌</div>Nothing has happened yet.</div>';

    $$('#ledger [data-undo]').forEach((b) => {
      b.onclick = async () => {
        try {
          const run = await rpc('journal.revert', { seq: Number(b.dataset.undo) });
          toast(run.status === 'completed' ? 'Undone.' : (run.message || 'Could not undo.'),
                run.status === 'completed' ? '' : 'warn');
          refreshLedger();
          if (currentView() === 'files') openDir(state.cwd);
        } catch (e) { toast(e.message, 'bad'); }
      };
    });
  } catch (e) {
    $('#ledger').innerHTML = `<div class="empty-state">${esc(e.message)}</div>`;
  }
}

// --- system ----------------------------------------------------------------

function refreshSystem() {
  const s = state.status;
  if (!s) return;
  const hw = s.hardware || {};
  const gpus = (hw.gpus || []).map((g) => `${g.name} (${g.vendor}${g.vram_mb ? `, ${g.vram_mb} MB` : ''})`).join(', ');

  $('#sys-sub').textContent = hw.profile ? `profile: ${hw.profile}` : '';
  $('#sys-hw').innerHTML = `
    <div class="rows">
      ${kv('Processor', `${hw.cpu_model || '?'} · ${hw.cpus || '?'} cores`)}
      ${kv('Memory', `${((hw.ram_mb || 0) / 1024).toFixed(1)} GB`)}
      ${kv('Free disk', `${((hw.disk_free_mb || 0) / 1024).toFixed(1)} GB`)}
      ${kv('Graphics', gpus || 'none detected')}
      ${kv('Kernel', s.system.kernel || '?')}
    </div>
    <p class="muted mt-14 fs-13">${esc(hw.explain || '')}</p>
    ${(hw.notes || []).map((n) => `<p class="faint mt-8 fs-125">${esc(n)}</p>`).join('')}`;

  const models = s.models || {};
  const creds = (models.credentials && models.credentials.providers) || [];
  $('#sys-models').innerHTML = `
    <div class="rows">
      ${(models.backends || []).map((b) => kv(b.name,
        `${b.model} — ${b.available ? '<span class="ok-text">reachable</span>' : '<span class="faint">not configured</span>'}`,
        true)).join('')}
    </div>
    <p class="faint mt-14 fs-125">
      Route: ${esc((models.route || []).join(' → '))}<br>
      Small requests: ${esc((models.route_small || []).join(' → '))} — routine work stays on this machine.
    </p>
    <p class="faint mt-10 fs-125">
      Keys: ${creds.filter((c) => c.configured).map((c) => esc(c.provider)).join(', ') || 'none set'} ·
      add one with <span class="num">nousctl key set &lt;provider&gt;</span>
    </p>`;
}

const kv = (k, v, raw = false) => `
  <div class="row"><span class="name faint kv-key">${esc(k)}</span>
  <span class="name">${raw ? v : esc(v)}</span></div>`;

// ----------------------------------------------------------------- events

function connectEvents() {
  const ws = new WebSocket(`ws://${location.host}/events`);

  ws.onopen = () => {
    $('#live-dot').classList.remove('off');
    $('#live-text').textContent = 'live';
  };

  ws.onclose = () => {
    $('#live-dot').classList.add('off');
    $('#live-text').textContent = 'reconnecting';
    // The desktop must survive the daemon restarting under it.
    setTimeout(connectEvents, 1500);
  };

  ws.onmessage = (msg) => {
    let frame;
    try { frame = JSON.parse(msg.data); } catch { return; }

    if (frame.topic === 'hello') {
      applyStatus(frame.data);
      return;
    }
    const data = frame.data || {};

    if (frame.topic === 'sensor' && data.metrics) {
      state.status = { ...state.status, metrics: data.metrics };
      if (currentView() === 'home') refreshHome();
    }
    if (frame.topic === 'notify' && data.alert) {
      toast(data.alert.message, data.alert.severity >= 4 ? 'bad' : 'warn');
    }
    if (frame.topic === 'intent' && data.phase === 'step') {
      const el = $(`.step[data-id="${CSS.escape(data.step || '')}"]`);
      if (el) { el.classList.remove('running'); el.classList.add('done'); }
    }
  };
}

function applyStatus(s) {
  state.status = s;
  state.home = (s.system && s.system.home) || null;
  $('#stat-version').textContent = s.version || '–';
  $('#stat-rules').textContent = `${s.policy_rules ?? '–'} rules`;
  $('#stat-journal').textContent = `${s.journal_entries ?? 0} entries`;
  $('#stat-profile').textContent = (s.hardware && s.hardware.profile) || '';
  refreshHome();
  if (currentView() === 'system') refreshSystem();
}

// -------------------------------------------------------------------- boot

async function boot() {
  $$('.ctx').forEach((c) => { c.onclick = () => showView(c.dataset.view); });

  $('#intent-form').onsubmit = async (e) => {
    e.preventDefault();
    const input = $('#intent-input');
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    try {
      renderPlan(await rpc('intent.plan', { text, context: state.context }));
    } catch (err) {
      toast(err.message, 'bad');
    }
  };

  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') { e.preventDefault(); $('#intent-input').focus(); }
    if (e.key === 'Escape') {
      // In the overlay, Escape means "go away" -- unless there is a plan on
      // screen, in which case it means "not that", and a second press dismisses.
      if (state.overlay && !state.plan) { window.close(); return; }
      $('#plan-host').innerHTML = '';
      state.plan = null;
    }
  });

  $('#btn-scan').onclick = scanNow;
  $('#btn-reindex').onclick = reindexMedia;
  $('#btn-undo').onclick = async () => {
    try {
      const run = await rpc('journal.revert', {});
      toast(run.status === 'completed' ? 'Undone.' : (run.message || 'Nothing to undo.'),
            run.status === 'completed' ? '' : 'warn');
      refreshLedger();
    } catch (e) { toast(e.message, 'bad'); }
  };
  $('#btn-render').onclick = () => toast('Say “render my edit” to compile the timeline.');
  $$('[data-media]').forEach((b) => { b.onclick = () => mediaControl(b.dataset.media); });

  // What kind of window is this? Decided before anything else, because the
  // overlay and the full shell want almost nothing in common.
  const params = new URLSearchParams(location.search);
  if (params.get('mode') === 'overlay') {
    state.overlay = true;
    document.body.classList.add('overlay');
    $('#overlay-hint').hidden = false;
    $('#intent-input').placeholder = 'What would you like to do?';
    state.context = {
      focus: params.get('focus') || '',
      cwd: params.get('cwd') || '',
      // Paths arrive newline-separated: a filename may contain almost anything
      // else, including commas.
      paths: (params.get('paths') || '').split('\n').filter(Boolean),
    };
    renderContext();
  }

  try {
    applyStatus(await rpc('sys.status'));
  } catch {
    toast('The daemon is not answering.', 'bad');
  }

  // The overlay stops here: no library, no file listing, no event stream. It is
  // on screen for a few seconds and should cost nothing.
  if (state.overlay) {
    const ask = params.get('ask');
    if (ask) {
      $('#intent-input').value = ask;
      $('#intent-form').requestSubmit();
    }
    $('#intent-input').focus();
    return;
  }

  // Files opens at home; the daemon reports it via sys.info.
  try {
    const info = await invoke('fs.list:~', { path: '~' }, { why: 'open home' });
    state.home = firstValue(info).path;
  } catch { /* the folder view will report it when opened */ }

  connectEvents();

  // Deep links. `?view=ledger` opens a context directly and `?ask=...` submits
  // an intent on load -- how the greeter hands off to the shell, and how a
  // notification points at the thing it is about.
  const view = params.get('view');
  if (view && $(`#view-${view}`)) showView(view);

  const ask = params.get('ask');
  if (ask) {
    $('#intent-input').value = ask;
    $('#intent-form').requestSubmit();
  } else {
    $('#intent-input').focus();
  }

  if (!view || view === 'home') scanNow();
}

boot();
