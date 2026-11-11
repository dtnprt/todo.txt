import * as T from './todotxt.js';
import { createComposer, attachHighlight, attachNoteHighlight, attachNoteTrigger, deleteWord, checkpoint, wordAt, dateAssist, replaceWordAt } from './composer.js';
import { buildCalendar, parseIsoDate } from './calendar.js';
import { createWebdav, isRemote, serverFor, davParent, davJoin, davName } from './webdav.js';
import { isAndroid, storageDirs, openFiles, initBackButton } from './android.js';

const $ = (s) => document.querySelector(s);
const STORE_KEY = 'todotxt.v1';
const hasFSA = 'showOpenFilePicker' in window;
// Desktop (Tauri) app: native dialogs and file access instead of the browser APIs.
const tauri = window.__TAURI__;
const isTauri = !!(tauri && tauri.dialog && tauri.fs);
const joinPath = (dir, name) => (/^https?:\/\//i.test(dir) ? davJoin(dir, name) : `${dir.replace(/[\\/]+$/, '')}/${name}`);
/** Desktop with a folder on this machine (git works there; a WebDAV folder has no git). */
const localFolder = () => isTauri && !!state.folder && !isRemote(state.folder);
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);
const hasDirPicker = 'showDirectoryPicker' in window;
const TXT_FILTER = [{ name: 'Text files', extensions: ['txt'] }];
const baseName = (p) => p.split(/[\\/]/).pop();

// One theme = one CSS file in web/themes/NAME.css (body class `theme-NAME`). To add a theme: drop the file there and add its name here.
const THEMES = [];   // built-in themes were moved out of web/: see themes/ (bundled with the desktop app, loaded like own themes)
// Own themes (desktop): `~/.todo.txt/themes/NAME.css`, same format as themes/*.css in the repo. Stored as `user:NAME`.
let userThemes = {};   // name -> css text
const themeClass = (t) => 'theme-' + t.replace(/^user:/, '').replace(/[^\w-]/g, '-');   // own themes use the same body class as the built-in one of that name (their artwork is `body.theme-NAME`)
// Theme families: the list shows only the family name, the variant (light/dark/...) is chosen in a second dropdown.
// [variant label, theme file]; the first entry is the default. Own themes named `NAME-light` / `NAME-dark` are grouped the same way.
const THEME_FAMILIES = {
  catppuccin: [['Mocha', 'catppuccin-mocha'], ['Macchiato', 'catppuccin-macchiato'], ['Frappé', 'catppuccin-frappe'], ['Latte', 'catppuccin-latte']],
  iceberg: [['Dark', 'iceberg'], ['Light', 'iceberg-light']],
  nord: [['Dark', 'nord'], ['Light', 'nord-light']],
  onehalf: [['Dark', 'onehalf-dark'], ['Light', 'onehalf-light']],
  rosepine: [['Main', 'rosepine'], ['Moon', 'rosepine-moon'], ['Dawn', 'rosepine-dawn']],
  solarized: [['Dark', 'solarized-dark'], ['Light', 'solarized-light']],
  tokyonight: [['Night', 'tokyonight'], ['Storm', 'tokyonight-storm'], ['Day', 'tokyonight-day']],
  tomorrow: [['Night', 'tomorrow-night'], ['Day', 'tomorrow']],
};
/** family key of a theme value ('nord-light' -> 'nord', 'user:x-dark' -> 'user:x', else the value itself) */
function themeKey(t) {
  for (const [k, v] of Object.entries(THEME_FAMILIES)) if (v.some(([, f]) => f === t)) return k;
  if (t.startsWith('user:')) {
    for (const [k, v] of Object.entries(THEME_FAMILIES)) if (v.some(([, f]) => 'user:' + f === t)) return 'user:' + k;
  }
  const m = /^user:(.+)-(light|dark)$/.exec(t);
  return m ? 'user:' + m[1] : t;
}
/** variants [[label, value]] of a family key (empty/1 entry = no variants) */
function themeVariants(key) {
  if (THEME_FAMILIES[key]) return THEME_FAMILIES[key];
  if (key.startsWith('user:')) {
    const base = key.slice(5);
    if (THEME_FAMILIES[base]) return THEME_FAMILIES[base].filter(([, f]) => userThemes[f] !== undefined).map(([l, f]) => [l, 'user:' + f]);
    return ['dark', 'light'].filter((v) => userThemes[`${base}-${v}`] !== undefined).map((v) => [v[0].toUpperCase() + v.slice(1), `user:${base}-${v}`]);
  }
  return [];
}

function applyTheme() {
  let link = document.getElementById('themeLink');
  const user = state.theme.startsWith('user:');
  let style = document.getElementById('themeStyle');
  if (user && userThemes[state.theme.slice(5)] !== undefined) {
    if (!style) { style = el('style', { id: 'themeStyle' }); document.head.append(style); }
    if (style.textContent !== userThemes[state.theme.slice(5)]) style.textContent = userThemes[state.theme.slice(5)];
  } else style?.remove();
  const want = state.theme && !user ? `themes/${state.theme}.css` : '';
  if (!want) { link?.remove(); }
  else if (!link) { link = el('link', { id: 'themeLink', rel: 'stylesheet', href: want }); document.head.append(link); }
  else if (link.getAttribute('href').split('?')[0] !== want) link.setAttribute('href', want);
  document.body.className = document.body.className.split(/\s+/).filter((c) => c && !c.startsWith('theme-')).concat(state.theme ? [themeClass(state.theme)] : []).join(' ');
  document.querySelectorAll('[data-theme]').forEach((b) => b.setAttribute('aria-checked', String(b.dataset.theme === themeKey(state.theme))));
}

let statusHover = null;   // status bar span showing the dates of the hovered entry
const state = {
  lines: [],
  undo: [],
  path: null,           // file path when running inside Tauri
  files: [],            // known file paths (desktop; `files=` JSON array in todo.cfg)
  folder: null,         // folder mode: folder path (desktop) or folder name (browser, see dirHandle)
  dirHandle: null,      // FileSystemDirectoryHandle of the folder (browser, File System Access API)
  folderFiles: [],      // visible *.txt file names in the folder
  git: null,            // desktop + folder mode: settings of .todo-git.cfg (see git_info), else null
  davState: null,      // WebDAV offline cache state (status bar), null when not a remote file
  gitStatus: null,     // {label, summary, ...} from git_status (status bar, left), null if not a repo
  gitMsg: '',           // last git message, shown in the status bar for a while
  handle: null,        // FileSystemFileHandle when opened/saved via File System Access API
  fileName: 'todo.txt',
  dirty: false,
  filter: { q: '', project: [], context: [], priority: [] },
  showDone: true,
  sidebar: !isAndroid,   // Android: the filter drawer, closed at start
  calendar: true,
  minimal: false,
  statusBar: true,
  touchBar: isAndroid || (typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches),   // helper bar under the input (touch screens); Settings -> General
  onTop: false,         // desktop only (todo.cfg `ontop=`)
  theme: '',            // name of a file in web/themes (without .css); '' = built-in light/dark
  autoSave: isAndroid, // desktop + Android; stored in todo.cfg (Android: on by default)
  frameless: false,    // desktop only; stored in todo.cfg (`frameless=`)
  showDates: false,    // show created/done dates inside the entries (else only tooltip + status bar on hover)
  lineNumbers: false,  // desktop only; stored in todo.cfg (`linenumbers=`)
  sort: 'priority',
  view: 'list',
  editing: null,       // line index being edited
};

// ---------- persistence (localStorage is a working copy, not the file) ----------

function persist() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify({
      lines: state.lines, fileName: state.fileName, path: state.path, folder: isTauri ? state.folder : null, dirty: state.dirty,
      sort: state.sort, showDone: state.showDone, sidebar: state.sidebar, calendar: state.calendar, minimal: state.minimal, statusBar: state.statusBar, touchBar: state.touchBar, showDates: state.showDates, theme: state.theme,
    }));
  } catch { /* storage unavailable or full: ignore */ }
  scheduleAutoSave();
}

function restore() {
  try {
    const s = JSON.parse(localStorage.getItem(STORE_KEY) || 'null');
    if (!s) return;
    state.lines = Array.isArray(s.lines) ? s.lines : [];
    state.fileName = s.fileName || 'todo.txt';
    state.dirty = !!s.dirty;
    state.path = isTauri && typeof s.path === 'string' ? s.path : null;
    state.folder = isTauri && !isAndroid && typeof s.folder === 'string' ? s.folder : null;   // no folder mode on Android
    state.sort = s.sort || 'priority';
    state.showDone = s.showDone !== false;
    state.sidebar = isAndroid ? false : s.sidebar !== false;
    state.calendar = s.calendar !== false;
    state.minimal = !!s.minimal;
    state.statusBar = s.statusBar !== false;
    if (typeof s.touchBar === 'boolean') state.touchBar = s.touchBar;
    if (typeof s.showDates === 'boolean') state.showDates = s.showDates;
    if (s.theme === 'zelda') s.theme = 'sheikah';   // renamed
    state.theme = isTauri && typeof s.theme === 'string' && s.theme ? (s.theme.startsWith('user:') ? s.theme : 'user:' + (s.theme === 'zelda' ? 'sheikah' : s.theme)) : '';
  } catch { /* corrupted: start empty */ }
}

// ---------- state changes ----------

function commit(newLines) {
  state.undo.push(state.lines);
  if (state.undo.length > 200) state.undo.shift();
  state.lines = newLines;
  state.dirty = true;
  persist();
  render();
}

function undo() {
  if (!state.undo.length) return;
  state.lines = state.undo.pop();
  state.dirty = true;
  state.editing = null;
  persist();
  render();
}

const replaceLine = (i, raw) => commit(state.lines.map((l, j) => (j === i ? raw : l)));
const removeLine = (i) => {
  const gone = state.lines[i];
  commit(state.lines.filter((_, j) => j !== i));
  deleteNoteOf(gone);
};

// Desktop: last known content of state.path on disk, used to detect external changes.
let diskText = null;
// WebDAV: ETag of the version read last (a save is refused if the file changed on the server meanwhile).
let davEtag = null;
let davCheckedAt = 0;
let davServers = [];   // `USER@URL` entries (todo.cfg `webdav=`)

const davEntry = (url) => serverFor(davServers, url)?.entry;
/** Text of a local or WebDAV file: {text, etag, missing}. */
async function readAny(p) {
  if (isRemote(p)) {
    const entry = davEntry(p);
    if (!entry) throw new Error('No WebDAV server is configured for this address (Settings → WebDAV).');
    return invoke('webdav_get', { entry, url: p });
  }
  return { text: await tauri.fs.readTextFile(p), etag: null, missing: false };
}
/** Write a local or WebDAV file; for WebDAV the new ETag is remembered. */
async function writeAny(p, text) {
  if (!isRemote(p)) return tauri.fs.writeTextFile(p, text);
  const entry = davEntry(p);
  if (!entry) throw new Error('No WebDAV server is configured for this address (Settings → WebDAV).');
  let r;
  try {
    r = await invoke('webdav_put', { entry, url: p, text, etag: davEtag });
  } catch (e) {
    if (!String(e).startsWith('conflict:')) throw e;
    if (!confirm('The file was changed on the server meanwhile.\nOverwrite it with your version? (Cancel keeps the server version; use File → Reload from disk to load it.)')) throw Object.assign(new Error('not saved'), { name: 'AbortError' });
    r = await invoke('webdav_put', { entry, url: p, text, etag: null });
  }
  davEtag = r.etag;
  if (r.merged != null) {
    // the server had changed while we were offline: the offline cache merged both versions
    const newLines = T.parseFile(r.merged);
    const diff = lineDiff(state.lines, newLines);
    commit(newLines);
    diskText = r.merged;
    persist(); render();
    showServerDiff(diff, false);
  }
  refreshDavState();
}

/** Offline cache state of the open WebDAV file (status bar): {offline, pending: [removed, added] | null}. */
async function refreshDavState() {
  let st = null;
  if (isTauri && state.path && isRemote(state.path)) {
    try { st = await invoke('webdav_state', { url: state.path }); } catch { st = null; }
  }
  if (JSON.stringify(st) !== JSON.stringify(state.davState)) { state.davState = st; render(); }
}

function loadText(text, name, handle = null, path = null, etag = null) {
  davEtag = etag;
  state.lines = T.parseFile(text);
  state.undo = [];
  state.fileName = name;
  state.handle = handle;
  // shared with the CLI/TUI: they follow the file chosen here - but not a file from a folder unknown to them
  if (isTauri && path && path !== state.path) isKnownFile(path).then((k) => { if (k) setCfg('lastfile', path); });
  state.path = path;
  diskText = path || handle ? text : null;
  state.dirty = false;
  state.editing = null;
  state.filter = { q: '', project: [], context: [], priority: [] };
  composer.clearSearch();
  persist();
  render();
}

// ---------- file I/O ----------

function confirmDiscard() {
  return !state.dirty || confirm('You have unsaved changes. Replace them?');
}

// ---------- version (window title) ----------

let appVersion = '';
async function loadVersion() {
  try {
    if (isTauri) appVersion = await invoke('app_version');
    else {
      const r = await fetch('version.json', { cache: 'no-store' });   // written by build.ps1, optional
      if (r.ok) { const v = await r.json(); appVersion = `${v.version} (${v.hash})`; }
    }
  } catch { /* no version known */ }
  render();
}

// ---------- git (desktop, folder mode; settings live in .todo-git.cfg in the folder) ----------
// Without that file nothing happens automatically. Pull on start/open, commit + push after saving.

let gitMsgTimer = null;
function setGitMsg(msg) {
  state.gitMsg = msg;
  clearTimeout(gitMsgTimer);
  gitMsgTimer = setTimeout(() => { state.gitMsg = ''; render(); }, 10000);
  render();
}

/** Branch + current commit for the status bar (cheap; polled and after saves/sync). */
async function refreshGitStatus() {
  let st = null;
  if (localFolder()) {
    try { st = await invoke('git_status', { dir: state.folder }); } catch { /* ignore */ }
  }
  if (JSON.stringify(st) !== JSON.stringify(state.gitStatus)) { state.gitStatus = st; render(); }
}

async function loadGit() {
  state.git = null;
  if (localFolder()) {
    try { state.git = await invoke('git_info', { dir: state.folder }); } catch { /* not available */ }
  }
  render();
  refreshGitStatus();
}

/** Folder opened / app started: pull before the data is read (only if .todo-git.cfg says so). */
async function gitStart() {
  if (!localFolder()) return;
  try {
    const m = await invoke('git_start', { dir: state.folder });
    if (m) setGitMsg(m);
  } catch (e) { setGitMsg(`git: ${e.message || e}`); }
  loadGit();
}

/** A file was written: commit and push as configured (the push runs in the background). */
async function gitChanged(name) {
  if (!localFolder() || !state.git || !state.git.enabled || !state.git.auto) return;
  try {
    const m = await invoke('git_changed', { dir: state.folder, name });
    if (m) setGitMsg(m);
  } catch (e) { setGitMsg(`git: ${e.message || e}`); }
  refreshGitStatus();
}

function renderGit() {
  const show = localFolder();
  const g = state.git;
  $('#gitDd').hidden = !(show && g && g.repo);
  $('#gitInit').style.display = show ? '' : 'none';
  if (!show) return;
  for (const k of ['enabled', 'auto', 'pull', 'commit', 'push']) {
    const b = $('#git' + k[0].toUpperCase() + k.slice(1));
    b.setAttribute('aria-checked', String(!!(g && g[k])));
    b.disabled = !(g && g.configured);
  }
  $('#gitInit').style.display = g && g.configured ? 'none' : '';
  $('#gitSync').disabled = !(g && g.repo) || (g.configured && !g.enabled);
}

for (const k of ['enabled', 'auto', 'pull', 'commit', 'push']) {
  $('#git' + k[0].toUpperCase() + k.slice(1)).onclick = async () => {
    if (!state.git) return;
    try {
      state.git = await invoke('git_set', { dir: state.folder, key: k, on: !state.git[k] });
      render();
    } catch (e) { alert(`git: ${e.message || e}`); }
  };
}
$('#gitInit').onclick = async () => {
  try { setGitMsg(`git: ${await invoke('git_init', { dir: state.folder })}`); } catch (e) { alert(`git: ${e.message || e}`); }
  loadGit();
};
$('#gitSync').onclick = async () => {
  setGitMsg('git: syncing…');
  try { setGitMsg(`git: ${await invoke('git_sync', { dir: state.folder })}`); } catch (e) { setGitMsg(`git: ${e.message || e}`); }
  checkExternalChange();
  refreshGitStatus();
};

// ---------- folder mode: a whole folder as storage, switch between its *.txt files ----------

/** Leave folder mode (a single file was opened). */
function leaveFolder() {
  if (!state.folder && !state.dirHandle) return;
  state.folder = null;
  state.dirHandle = null;
  state.folderFiles = [];
  state.git = null;
  setCfg('folder', '');
}

/** Refresh state.folderFiles: visible (not hidden) *.txt files, minus the note files. True if changed. */
async function listFolder() {
  let names = null;
  try {
    if (isTauri && state.folder && isRemote(state.folder)) {
      const items = await invoke('webdav_list', { entry: davEntry(state.folder), url: state.folder });
      names = items.filter((i) => !i.is_dir && /\.txt$/i.test(i.name) && !i.name.startsWith('.')).map((i) => i.name)
        .sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
    } else if (isTauri && state.folder) {
      names = await invoke('list_txt_files', { dir: state.folder });
    } else if (state.dirHandle) {
      const found = [];
      for await (const [name, h] of state.dirHandle.entries()) {
        if (h.kind === 'file' && /\.txt$/i.test(name) && !name.startsWith('.')) {
          found.push({ name, text: await (await h.getFile()).text() });
        }
      }
      const refs = new Set();
      for (const f of found) for (const m of f.text.matchAll(/(?:^|\s)note:(\S+)/g)) refs.add(m[1].toLowerCase());
      names = found.map((f) => f.name).filter((n) => !refs.has(n.slice(0, -4).toLowerCase()))
        .sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
    }
  } catch (e) { console.warn('Could not list folder:', e); }
  if (names && JSON.stringify(names) !== JSON.stringify(state.folderFiles)) {
    state.folderFiles = names;
    return true;
  }
  return false;
}

async function refreshFolder() {
  if (localFolder()) {
    try { const r = await invoke('git_result'); if (r) setGitMsg(r); } catch { /* ignore */ }
  }
  if ((state.folder || state.dirHandle) && (await listFolder())) renderFileList();
}

/** Open a file of the current folder by name (a missing todo.txt is created on the first save). */
async function openInFolder(name) {
  if (isTauri) {
    const p = joinPath(state.folder, name);
    let text = '', etag = null;
    try { const r = await readAny(p); text = r.missing ? '' : r.text; etag = r.etag || null; } catch (e) { if (isRemote(p)) throw e; /* new local file */ }
    loadText(text, name, null, p, etag);
    setCfg('file', p);
  } else {
    const h = await state.dirHandle.getFileHandle(name, { create: true });
    loadText(await (await h.getFile()).text(), name, h);
  }
}

async function switchFolderFile(name) {
  if (name === state.fileName || !confirmDiscard()) return;
  try { await openInFolder(name); } catch (e) { alert(`Could not open ${name}: ${e.message || e}`); }
}

/** WebDAV folder mode: a folder URL on a server (all its .txt files can be switched between). */
async function openFolderUrl(url) {
  if (!confirmDiscard()) throw new Error('cancelled');
  state.folder = url.endsWith('/') ? url : `${url}/`;
  state.dirHandle = null;
  state.git = null;
  setCfg('folder', state.folder);
  state.folderFiles = [];
  await listFolder();
  await openInFolder(state.folderFiles.includes('todo.txt') ? 'todo.txt' : state.folderFiles[0] || 'todo.txt');
}

async function openFolder() {
  if (!confirmDiscard()) return;
  try {
    if (isTauri) {
      const dir = await tauri.dialog.open({ multiple: false, directory: true });
      if (!dir) return;
      state.folder = dir;
      state.dirHandle = null;
      setCfg('folder', dir);
      await gitStart();
    } else if (hasDirPicker) {
      state.dirHandle = await window.showDirectoryPicker({ mode: 'readwrite' });
      state.folder = state.dirHandle.name;
    } else {
      return alert('This browser cannot open folders.');
    }
    state.folderFiles = [];
    await listFolder();
    await openInFolder(state.folderFiles.includes('todo.txt') ? 'todo.txt' : state.folderFiles[0] || 'todo.txt');
  } catch (e) {
    if (e.name !== 'AbortError') alert(`Could not open folder: ${e.message || e}`);
  }
}

async function openFile() {
  if (!confirmDiscard()) return;
  try {
    if (isTauri) {
      const p = await tauri.dialog.open({ multiple: false, directory: false, filters: TXT_FILTER });
      if (!p) return;
      leaveFolder();
      loadText(await tauri.fs.readTextFile(p), baseName(p), null, p);
      rememberFile(p);
    } else if (hasFSA) {
      const [handle] = await window.showOpenFilePicker({
        types: [{ description: 'Text files', accept: { 'text/plain': ['.txt'] } }],
      });
      const file = await handle.getFile();
      leaveFolder();
      loadText(await file.text(), file.name, handle);
    } else {
      $('#fileInput').click();
    }
  } catch (e) {
    if (e.name !== 'AbortError') alert(`Could not open file: ${e.message}`);
  }
}

async function openFromFileObject(file) {
  if (!confirmDiscard()) return;
  leaveFolder();
  loadText(await file.text(), file.name);
}

/** Save text under a user-chosen name. Resolves to false if the user cancelled. */
async function download(name, text) {
  if (isAndroid && state.path && !isRemote(state.path)) {
    const p = state.path.replace(/[^/\\]*$/, '') + name;   // done.txt next to the list (appended)
    let old = '';
    try { old = await tauri.fs.readTextFile(p); } catch { /* new file */ }
    await tauri.fs.writeTextFile(p, old + text);
    return true;
  }
  if (isTauri) {
    const p = await tauri.dialog.save({ defaultPath: name, filters: TXT_FILTER });
    if (!p) return false;
    await tauri.fs.writeTextFile(p, text);
    return true;
  }
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  return true;
}

async function save({ as = false } = {}) {
  applySortToLines();
  const text = T.serializeFile(state.lines);
  try {
    if (isTauri) {
      let p = state.path;
      if (as || !p) {
        p = await tauri.dialog.save({ defaultPath: state.fileName, filters: TXT_FILTER });
        if (!p) return;
        state.path = p;
        state.fileName = baseName(p);
        davEtag = null;
        rememberFile(p, true);
      }
      await writeAny(p, text);
      diskText = text;
      if (!isRemote(p)) gitChanged(baseName(p));
    } else if (hasFSA) {
      if (as || !state.handle) {
        state.handle = await window.showSaveFilePicker({
          suggestedName: state.fileName,
          types: [{ description: 'Text files', accept: { 'text/plain': ['.txt'] } }],
        });
        state.fileName = state.handle.name;
      }
      const w = await state.handle.createWritable();
      await w.write(text);
      await w.close();
    } else {
      await download(state.fileName, text);
    }
    state.dirty = false;
    persist();
    render();
  } catch (e) {
    if (e.name !== 'AbortError') alert(`Could not save: ${e.message}`);
  }
}

/** Lines removed / added between two versions (order-independent: the list may be sorted differently). */
function lineDiff(oldLines, newLines) {
  const count = (arr) => arr.reduce((m, l) => m.set(l, (m.get(l) || 0) + 1), new Map());
  const o = count(oldLines), n = count(newLines);
  const removed = [], added = [];
  for (const l of oldLines) { const k = n.get(l) || 0; if (k) n.set(l, k - 1); else removed.push(l); }
  for (const l of newLines) { const k = o.get(l) || 0; if (k) o.set(l, k - 1); else added.push(l); }
  // the lines both versions have: in the same order? (the server list may simply be sorted differently)
  const keep = (arr, other) => { const m = count(other); return arr.filter((l) => { const k = m.get(l) || 0; if (k) { m.set(l, k - 1); return true; } return false; }); };
  const a = keep(oldLines, newLines), b = keep(newLines, oldLines);
  const reordered = a.some((l, i) => l !== b[i]);
  return { removed, added, reordered };
}
/**
 * Overlay with the difference between the local list and the version on the WebDAV server.
 * `ask`: there are unsaved local changes -> buttons "Load server version" / "Keep mine", resolves true = load.
 * Otherwise the server version was already loaded (undoable): informational, resolves on close.
 */
function showServerDiff({ removed, added, reordered }, ask) {
  return new Promise((resolve) => {
    const rows = (lines, cls, sign) => lines.map((l) => el('div', { className: 'diff-line ' + cls }, el('span', { className: 'diff-sign', textContent: sign }), el('span', { textContent: l })));
    const close = (v) => { document.removeEventListener('keydown', onKey, true); ov.remove(); resolve(v); };
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(false); } };
    const body = el('div', { className: 'diff-body' }, ...rows(removed, 'del', '−'), ...rows(added, 'add', '+'));
    if (!removed.length && !added.length) body.append(el('div', { className: 'set-hint', textContent: 'No lines were added or removed.' }));
    const sortNote = reordered ? el('div', { className: 'set-hint diff-sort', textContent: (removed.length || added.length ? 'The remaining lines are also sorted differently on the server.' : 'The lines are only sorted differently on the server.') }) : null;
    const buttons = ask
      ? [el('button', { type: 'button', className: 'primary', textContent: 'Load server version', onclick: () => close(true) }),
         el('button', { type: 'button', textContent: 'Keep mine', onclick: () => close(false) })]
      : [el('button', { type: 'button', className: 'primary', textContent: 'OK', onclick: () => close(false) })];
    const ov = el('div', { className: 'help dav-diff', onmousedown: (e) => { if (e.target === ov) close(false); } },
      el('div', { className: 'help-box diff-box', role: 'dialog', 'aria-label': 'Changed on server' },
        el('h2', { textContent: 'The file was changed on the server' }),
        el('div', { className: 'set-hint', textContent: ask
          ? `${removed.length} line(s) removed, ${added.length} added there. You have unsaved changes: load the server version (discards them) or keep yours (saving overwrites the server).`
          : `${removed.length} line(s) removed, ${added.length} added. The server version was loaded (Undo restores yours).` }),
        ...(sortNote ? [sortNote] : []), body, el('div', { className: 'diff-actions' }, ...buttons)));
    document.addEventListener('keydown', onKey, true);
    document.body.append(ov);
    buttons[0].focus();
  });
}

// Reload the open file when another program changed it (desktop: polled), or on demand (Refresh button).
let checking = false;
let manualCheck = false;
async function readCurrent() {
  if (isTauri && state.path) {
    if (isRemote(state.path)) {
      // cheap check first: only fetch the text when the ETag changed
      const entry = davEntry(state.path);
      if (!entry) return null;
      if (!manualCheck && davEtag) {
        if (Date.now() - davCheckedAt < 8000) return null;
        davCheckedAt = Date.now();
        const tag = await invoke('webdav_etag', { entry, url: state.path });
        refreshDavState();
        if (tag === davEtag || tag === null) return null;
      }
      const r = await invoke('webdav_get', { entry, url: state.path });
      refreshDavState();
      if (r.missing) return null;
      davEtag = r.etag;
      return r.text;
    }
    return tauri.fs.readTextFile(state.path);
  }
  if (state.handle) return (await state.handle.getFile()).text();
  return null;
}
async function checkExternalChange(manual = false) {
  if (manual !== true) manual = false;
  if (checking || (!manual && state.editing !== null)) return;
  if (!state.path && !state.handle) { if (manual) alert('No file is open.'); return; }
  checking = true;
  manualCheck = manual;
  try {
    const text = await readCurrent();
    if (text === null) return;
    const first = diskText === null; // path restored from the last session: no baseline yet
    if (!manual && text === diskText) return;
    diskText = text;
    const same = T.serializeFile(T.parseFile(text)) === T.serializeFile(state.lines);
    if (same) return;
    if (first && !manual && state.dirty) return; // keep unsaved work from last session
    const remote = !!state.path && isRemote(state.path);
    const newLines = T.parseFile(text);
    const diff = remote ? lineDiff(state.lines, newLines) : null;
    const wasDirty = state.dirty;
    if (wasDirty) {
      const load = remote ? await showServerDiff(diff, true) : confirm('The file was changed on disk. Reload it and discard your unsaved changes?');
      if (!load) return;
    }
    commit(newLines); // keeps an undo step
    state.dirty = false;
    persist();
    render();
    if (remote && !wasDirty) showServerDiff(diff, false);   // informational, not awaited
  } catch (e) {
    console.warn('Reload failed:', e);
    if (manual) alert(`Could not read file: ${e.message || e}`);
  } finally {
    checking = false;
  }
}
/** The terminal app (or the CLI) switched to another known file: open it here too (not with unsaved changes). */
let followBusy = false;
async function followLastFile() {
  if (!isTauri || followBusy || state.dirty || state.editing !== null) return;
  followBusy = true;
  try {
    const cfg = await tauri.fs.readTextFile(await cfgPath());
    const get = (k) => (cfg.match(new RegExp(`^\\s*${k}\\s*=\\s*(.+?)\\s*$`, 'm')) || [])[1];
    const norm = (x) => (x || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
    const last = get('lastfile');
    if (!last || norm(last) === norm(state.path)) return;
    let files = [];
    try { files = JSON.parse(get('files') || '[]'); } catch { /* ignore */ }
    const folder = get('folder');
    const inFolder = !!folder && norm(last).startsWith(norm(folder) + '/');
    if (!(inFolder || files.some((f) => norm(f) === norm(last)) || norm(get('file')) === norm(last))) return;   // unknown file: not followed
    const r = await readAny(last);
    if (r.missing || state.dirty) return;
    if (inFolder && norm(state.folder) !== norm(folder)) {
      state.folder = folder; state.dirHandle = null;
      await listFolder();
    }
    state.files = [...new Set([...state.files, ...files])];
    loadText(r.text, davFileName(last), null, last, r.etag);
    renderFileList();
  } catch { /* config not readable right now */ } finally { followBusy = false; }
}

if (isTauri) {
  setInterval(followLastFile, 1500);
  setInterval(checkExternalChange, 1500);
  setInterval(refreshFolder, 3000);
  setInterval(refreshGitStatus, 3000);
  window.addEventListener('focus', checkExternalChange);
}

// ---------- shared config with the terminal app (~/.todo.txt/todo.cfg, key=value lines) ----------

const cfgPath = () => invoke('cfg_path');   // ~/.todo.txt/todo.cfg (migrates the old ~/.config/todo.cfg)

/** Set `key=value` in todo.cfg, keeping all other lines. Writes run one after the other, otherwise two
 *  quick calls (e.g. `lastfile=` and `file=`) both read the old file and one of them gets lost. */
let cfgQueue = Promise.resolve();
function setCfg(key, value) {
  cfgQueue = cfgQueue.then(() => setCfgNow(key, value));
  return cfgQueue;
}
async function setCfgNow(key, value) {
  if (!isTauri || !tauri.path) return;
  try {
    const cp = await cfgPath();
    let lines = [];
    try { lines = (await tauri.fs.readTextFile(cp)).split(/\r?\n/); } catch { /* new config */ }
    const re = new RegExp(`^\\s*${key}\\s*=`);
    lines = lines.filter((l) => l !== '' && !re.test(l));
    lines.unshift(`${key}=${value}`);
    await tauri.fs.writeTextFile(cp, lines.join('\n') + '\n');
  } catch (e) { console.warn('Could not update todo.cfg:', e); }
}

/** A file the other clients know: in the files list, inside the configured folder, or next to the remembered file. */
async function isKnownFile(p) {
  if (!p) return false;
  const norm = (x) => x.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
  if (state.files.some((f) => norm(f) === norm(p))) return true;
  if (state.folder && norm(p).startsWith(norm(state.folder) + '/')) return true;
  const last = await cfgFile();
  return !!last && dirOf(last) === dirOf(p);
}

const dirOf = (p) => p.replace(/\\/g, '/').replace(/\/[^/]*$/, '').toLowerCase();

/** The `file=` currently stored in todo.cfg (or null). */
async function cfgFile() {
  if (!isTauri || !tauri.path) return null;
  try {
    const m = (await tauri.fs.readTextFile(await cfgPath())).match(/^\s*file\s*=\s*(.+?)\s*$/m);
    return m ? m[1] : null;
  } catch { return null; }
}

/** Remember the open file in todo.cfg (`file=`) and add it to the list of files (`files=["…","…"]`, JSON).
 *  A file outside the folder of the remembered one (e.g. the todo.txt of another git repository) is only
 *  opened, never remembered, unless `force` (switching to a file from the list). */
async function rememberFile(path, force = false) {
  if (!force) {
    const last = await cfgFile();
    if (last && dirOf(last) !== dirOf(path)) return;
  }
  setCfg('file', path);
  setCfg('lastfile', path);
  if (!state.files.includes(path)) {
    state.files.push(path);
    saveFiles();
  }
}
const saveFiles = () => setCfg('files', JSON.stringify(state.files));

// WebDAV (desktop): server list in todo.cfg (`webdav=["USER@URL"]`), file browser overlay, Settings pane (webdav.js)
const dav = createWebdav({
  invoke,
  servers: () => davServers,
  setServers: (list) => { davServers = list; setCfg('webdav', JSON.stringify(list)); },
  currentUrl: () => (isRemote(state.path) ? state.path : null),
  canSaveHere: () => true,
  openFolderUrl: (url) => openFolderUrl(url),
  openUrl: async (url) => {
    if (!confirmDiscard()) throw new Error('cancelled');
    const r = await readAny(url);
    leaveFolder();
    loadText(r.text, davName(url), null, url, r.etag);
    rememberFile(url);
  },
  saveAs: async (url) => {
    leaveFolder();
    state.path = url;
    state.fileName = davName(url);
    davEtag = null;
    rememberFile(url, true);
    await save();
  },
});

const davFileName = (p) => (isRemote(p) ? davName(p) : baseName(p));

async function switchFile(p) {
  if (p === state.path || !confirmDiscard()) return;
  try {
    leaveFolder();
    const r = await readAny(p);
    loadText(r.text, davFileName(p), null, p, r.etag);
    rememberFile(p, true);
  } catch (e) {
    alert(`Could not open ${p}: ${e.message || e}`);
  }
}

function removeFromFiles(p) {
  state.files = state.files.filter((f) => f !== p);
  saveFiles();
  renderFileList();
}

function renderFileList() {
  const btn = $('#fileDd .file-btn');
  const canFolder = isTauri || hasDirPicker;
  btn.disabled = !canFolder;
  const menu = $('#fileList');
  if (btn.disabled) return;
  const tick = () => el('span', { className: 'tick' }, el('i', { className: 'fa-solid fa-check' }));
  let rows = [];
  if (state.folder) {
    rows = [el('div', { className: 'file-head', title: state.folder },
      el('i', { className: 'fa-solid fa-folder' }), ' ', baseName(state.folder.replace(/[\\/]+$/, '')))];
    for (const name of state.folderFiles) {
      const pick = el('button', { className: 'file-pick', onclick: () => switchFolderFile(name) }, tick(), name);
      pick.setAttribute('role', 'menuitemradio');
      pick.setAttribute('aria-checked', String(name === state.fileName));
      rows.push(el('div', { className: 'file-row' }, pick));
    }
    if (!state.folderFiles.length) rows.push(el('div', { className: 'side-empty' }, 'No .txt files'));
  } else if (isTauri) {
    const files = state.path && !state.files.includes(state.path) ? [...state.files, state.path] : state.files;
    rows = files.map((p) => {
      const cur = p === state.path;
      const dir = p.slice(0, Math.max(0, p.length - baseName(p).length)).replace(/[\\/]+$/, '');
      const pick = el('button', { className: 'file-pick', title: p, onclick: () => switchFile(p) },
        tick(), baseName(p), el('small', {}, baseName(dir)));
      pick.setAttribute('role', 'menuitemradio');
      pick.setAttribute('aria-checked', String(cur));
      const rm = el('button', { className: 'file-rm', title: 'Remove from list', onclick: () => removeFromFiles(p) },
        el('i', { className: 'fa-solid fa-xmark' }));
      rm.dataset.keep = '';
      return el('div', { className: 'file-row' }, pick, rm);
    });
  }
  const open = el('button', { role: 'menuitem', onclick: openFile }, el('i', { className: 'fa-solid fa-folder-open' }), ' Open…');
  const openDir = el('button', { role: 'menuitem', onclick: openFolder }, el('i', { className: 'fa-solid fa-folder-tree' }), ' Open folder…');
  menu.replaceChildren(...rows, ...(rows.length ? [el('hr')] : []), open, ...(canFolder ? [openDir] : []));
}

// Auto save (desktop only): `autosave=true|false` in todo.cfg; saves to state.path shortly after each change.
let autoSaveTimer = null;
function scheduleAutoSave() {
  clearTimeout(autoSaveTimer);
  if (!isTauri || !state.autoSave || !state.dirty || !state.path) return;
  autoSaveTimer = setTimeout(() => {
    if (state.autoSave && state.dirty && state.path && state.editing === null) save();
  }, 600);
}

async function loadAutoSave() {
  if (!isTauri || !tauri.path) return;
  try {
    const cfg = await tauri.fs.readTextFile(await cfgPath());
    const flag = (key) => {
      const m = cfg.match(new RegExp(`^\\s*${key}\\s*=\\s*(\\S+)`, 'm'));
      return !!m && /^(true|1|yes|on)$/i.test(m[1]);
    };
    state.autoSave = /^\s*autosave\s*=/m.test(cfg) ? flag('autosave') : isAndroid;
    state.lineNumbers = flag('linenumbers');
    state.frameless = flag('frameless');
    const num = (key, def) => { const m = cfg.match(new RegExp(`^\\s*${key}\\s*=\\s*(\\d+)`, 'm')); return m ? Math.min(90, +m[1]) : def; };
    state.onTop = flag('ontop');
    if (/^\s*touchbar\s*=/m.test(cfg)) state.touchBar = flag('touchbar');
    if (/^\s*showdates\s*=/m.test(cfg)) state.showDates = flag('showdates');
    if (/^\s*statusbar\s*=/m.test(cfg)) state.statusBar = flag('statusbar');   // shared with the terminal app
  } catch { /* no config yet: off */ }
  render();
  scheduleAutoSave();
}

/** On startup: open the file the terminal app (or this app) used last. */
async function openLastFile() {
  if (!isTauri || !tauri.path) return;
  try {
    const home = isAndroid ? (storageDirs()[0]?.path || '') : (await tauri.path.homeDir()).replace(/[\\/]+$/, '');
    let target = null;
    try {
      const cfg = await tauri.fs.readTextFile(await cfgPath());
      const m = cfg.match(/^\s*file\s*=\s*(.+?)\s*$/m);
      if (m) target = m[1];
      const lm = cfg.match(/^\s*lastfile\s*=\s*(.+?)\s*$/m);   // the file used last by any client wins over file=
      if (lm) target = lm[1];
      const wm = cfg.match(/^\s*webdav\s*=\s*(.+?)\s*$/m);
      if (wm) { try { const a = JSON.parse(wm[1]); if (Array.isArray(a)) davServers = a.filter((x) => typeof x === 'string' && x); } catch { /* ignore */ } }
      const dm = cfg.match(/^\s*folder\s*=\s*(.+?)\s*$/m);
      if (dm && !isAndroid) state.folder = dm[1];
      const fm = cfg.match(/^\s*files\s*=\s*(.+?)\s*$/m);
      if (fm) {
        try {
          const arr = JSON.parse(fm[1]);
          if (Array.isArray(arr)) state.files = arr.filter((f) => typeof f === 'string' && f);
        } catch { /* malformed list: ignore */ }
      }
      if (target && !state.files.includes(target)) state.files.push(target);
    } catch { /* no config yet */ }
    const normDir = (x) => x.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
    // the last file lies outside the configured folder (or is remote): open it alone, keep folder= in the config
    if (state.folder && target && !normDir(target).startsWith(normDir(state.folder) + '/')) {
      state.folder = null;
      state.folderFiles = [];
    }
    if (state.folder) {
      if (localFolder()) await gitStart();
      await listFolder();
      const norm = (x) => x.replace(/\\/g, '/').replace(/\/+$/, '');
      if (!(target && norm(target).startsWith(norm(state.folder) + '/'))) {
        target = joinPath(state.folder, state.folderFiles.includes('todo.txt') ? 'todo.txt' : state.folderFiles[0] || 'todo.txt');
      }
    }
    renderFileList();
    const candidates = [target, `${home}/todo.txt`].filter(Boolean);
    if (state.dirty && state.path) return; // keep unsaved work from the last session
    for (const p of candidates) {
      let r;
      try { r = await readAny(p); } catch (e) { if (isRemote(p)) alert(`Could not open ${p}:
${e.message || e}`); continue; }
      if (r.missing) continue;
      if (p === state.path && r.text === diskText) return;
      loadText(r.text, davFileName(p), null, p, r.etag);
      return;
    }
    if (isAndroid && home) await androidCreateFile(`${home}/todo.txt`);   // first start: an empty todo.txt in the app storage
  } catch (e) { console.warn('Could not open last file:', e); }
}

async function archiveDone() {
  const done = state.lines.filter((l) => T.parse(l).done);
  if (!done.length) return;
  if (!confirm(`Save ${done.length} completed task(s) as done.txt and remove them from the list?`)) return;
  try {
    if (!(await download('done.txt', T.serializeFile(done)))) return;
  } catch (e) {
    return alert(`Could not archive: ${e.message}`);
  }
  commit(state.lines.filter((l) => !T.parse(l).done));
}

// ---------- note: longer text kept next to the todo file(s) ----------
// A task can have several notes (`note:Note-1 note:Note-2`); each is a file NAME.txt in the notes folder.
// Desktop: `<folder>/notes/NAME.txt` in folder mode, else `~/.todo.txt/notes/NAME.txt` (Rust commands);
// WebDAV file: `notes/NAME.txt` next to the todo file on the server.
// Browser: `notes/NAME.txt` in the opened folder, else localStorage.

const NOTE_KEY = 'todotxt.note.';
const onDav = () => isTauri && isRemote(state.path);
const noteUrl = (name) => davJoin(davParent(state.path) + 'notes/', `${name}.txt`);

async function readNote(name) {
  try {
    if (onDav()) return (await invoke('webdav_get', { entry: davEntry(state.path), url: noteUrl(name) })).text;
    if (isTauri) return await invoke('read_note', { folder: state.folder, name });
    if (state.dirHandle) return await (await (await (await state.dirHandle.getDirectoryHandle('notes')).getFileHandle(`${name}.txt`)).getFile()).text();
    return localStorage.getItem(NOTE_KEY + name) || '';
  } catch { return ''; }
}

async function writeNote(name, text) {
  if (onDav()) return invoke('webdav_put', { entry: davEntry(state.path), url: noteUrl(name), text, etag: null });
  if (isTauri) return invoke('write_note', { folder: state.folder, name, text });
  if (state.dirHandle) {
    const dir = await state.dirHandle.getDirectoryHandle('notes', { create: true });
    const h = await dir.getFileHandle(`${name}.txt`, { create: true });
    const w = await h.createWritable();
    await w.write(text);
    return w.close();
  }
  localStorage.setItem(NOTE_KEY + name, text);
}

async function deleteNoteFile(name) {
  try {
    if (onDav()) await invoke('webdav_delete', { entry: davEntry(state.path), url: noteUrl(name) });
    else if (isTauri) await invoke('delete_note', { folder: state.folder, name });
    else if (state.dirHandle) await (await state.dirHandle.getDirectoryHandle('notes')).removeEntry(`${name}.txt`);
    else localStorage.removeItem(NOTE_KEY + name);
    gitChanged(`${name}.txt`);
  } catch { /* no note file */ }
}

/** Delete the note files of a removed task (unless another task still uses the name). */
async function deleteNoteOf(raw) {
  const still = new Set(state.lines.flatMap((l) => T.noteNames(T.parse(l))));
  for (const name of T.noteNames(T.parse(raw))) if (!still.has(name)) await deleteNoteFile(name);
}

const noteNameOk = (n) => /^[\w.-]+$/.test(n) && !n.startsWith('.');
const escRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const allNoteNames = () => state.lines.flatMap((l) => T.noteNames(T.parse(l)));

/**
 * Overlay editor for a note (same box as Settings): a name field (default `Note-1`, `Note-2`, ...) and, when the
 * entry has notes, a dropdown of all of them (choosing one saves this note and opens that one).
 *   names: the entry's notes · taken(n): the name belongs to something else
 *   onSave(name, text) runs on save · onPick(name|null) after the dialog closed because another note (null = new) was chosen
 *   onClose(savedName|null) after it closes either way.
 */
function noteDialog({ name, names, isNew, text, taken, onSave, onPick, onClose }) {
  if (document.getElementById('noteDlg')) return;
  const ta = el('textarea', { className: 'note-text', value: text, spellcheck: false });
  const field = attachNoteHighlight(ta);
  const nameIn = el('input', { type: 'text', className: 'note-name', value: name, spellcheck: false, title: 'Note name (the file is NAME.txt)', 'aria-label': 'Note name' });
  const err = el('small', { className: 'note-err' });
  let savedName = null;
  const close = () => { ov.remove(); if (onClose) onClose(savedName); };
  const cancelClose = () => { savedName = null; close(); };
  /** Validate and save; true when the dialog may close. */
  const commitNote = async () => {
    const n = nameIn.value.trim();
    if (!noteNameOk(n)) { err.textContent = 'Name: letters, digits, _ - . only'; nameIn.focus(); return false; }
    if (taken(n)) { err.textContent = `"${n}" is already used by another note`; nameIn.focus(); return false; }
    if (isNew && !ta.value.trim()) return true;   // nothing typed: no note is created
    try { await onSave(n, ta.value); savedName = n; return true; } catch (e) { alert(`Could not save note: ${e.message || e}`); return false; }
  };
  const save = async () => { if (await commitNote()) close(); };
  const NEW = '\u0000new';
  let pick = null;
  if (names.length) {
    pick = el('select', { className: 'note-pick', title: 'All notes of this entry', 'aria-label': 'Notes of this entry' },
      ...names.map((n) => el('option', { value: n, textContent: n, selected: n === name && !isNew })),
      el('option', { value: NEW, textContent: '＋ New note', selected: isNew }));
    pick.onchange = async () => {
      const v = pick.value;
      if ((v === NEW && isNew) || (v === name && !isNew)) return;
      if (await commitNote()) { close(); if (onPick) onPick(v === NEW ? null : v); }
      else pick.value = isNew ? NEW : name;
    };
  }
  const ok = el('button', { className: 'primary', textContent: 'Save', onclick: save });
  const cancel = el('button', { textContent: 'Cancel', onclick: cancelClose });
  const ov = el('div', { id: 'noteDlg', className: 'help' },
    el('div', { className: 'help-box set-box note-box', role: 'dialog', 'aria-label': 'Note' },
      el('div', { className: 'set-head note-head' }, el('i', { className: 'fa-solid fa-note-sticky' }), nameIn, ...(pick ? [pick] : []), err, el('span', { className: 'spacer' }),
        el('button', { type: 'button', className: 'icon', title: 'Close (Esc)', 'aria-label': 'Close', onclick: cancelClose }, el('i', { className: 'fa-solid fa-xmark' }))),
      field.el,
      el('div', { className: 'note-buttons' }, el('small', {}, 'Ctrl+Enter saves · Esc cancels'), el('span', { className: 'spacer' }), cancel, ok)));
  ov.addEventListener('keydown', (e) => {
    e.stopPropagation();
    if (e.key === 'Escape') cancelClose();
    else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
  });
  ov.addEventListener('mousedown', (e) => { if (e.target === ov) cancelClose(); });
  document.body.append(ov);
  ta.focus();
}

/**
 * Edit a note of line `i`. `which`: a note name; null = a new note (named Note-1, Note-2, ...); undefined = the first
 * note, or a new one if the task has none.
 */
async function editNote(i, which) {
  if (document.getElementById('noteDlg')) return;
  const names = T.noteNames(T.parse(state.lines[i]));
  let name = which === undefined ? names[0] : which;
  const isNew = !name || !names.includes(name);
  if (isNew) name = T.nextNoteName(allNoteNames());
  const others = new Set(state.lines.flatMap((l, j) => (j === i ? [] : T.noteNames(T.parse(l)))).map((n) => n.toLowerCase()));
  noteDialog({
    name, names, isNew,
    text: isNew ? '' : await readNote(name),
    taken: (n) => (isNew || n.toLowerCase() !== name.toLowerCase()) && (others.has(n.toLowerCase()) || names.some((x) => x !== name && x.toLowerCase() === n.toLowerCase())),
    onSave: async (n, body) => {
      await writeNote(n, body);
      if (isNew) replaceLine(i, `${state.lines[i]} note:${n}`);
      else if (n !== name) {
        replaceLine(i, state.lines[i].replace(new RegExp(`(^|\\s)note:${escRe(name)}(?=\\s|$)`), `$1note:${n}`));
        if (!allNoteNames().includes(name)) await deleteNoteFile(name);
      }
      gitChanged(`${n}.txt`);
    },
    onPick: (v) => setTimeout(() => editNote(i, v), 0),
  });
}

/**
 * Typing `note:` in the add bar / edit field opens the note editor. On save the word becomes `note:NAME`
 * (the note file is written right away); on cancel or an empty note the word is removed. Typing continues after.
 * Notes already in the input are listed in the dropdown and can be edited too.
 */
function noteForInput(input) {
  if (document.getElementById('noteDlg')) return;
  const { start, end } = wordAt(input);
  const put = (r, word) => {
    checkpoint(input);
    const v = input.value;
    const before = v.slice(0, r.start);
    const after = v.slice(r.end).replace(/^\s+/, '');
    const sep = word && before && !/\s$/.test(before) ? ' ' : '';
    const ins = word ? `${sep}${word} ` : '';
    input.value = before + ins + after;
    const caret = before.length + ins.length;
    input.setSelectionRange(caret, caret);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const inputNames = (v) => v.split(/\s+/).map((w) => /^note:([\w.-]+)$/.exec(w)?.[1]).filter((n) => n && !n.startsWith('.'));
  const wordRange = (n) => {
    const m = new RegExp(`(^|\\s)note:${escRe(n)}(?=\\s|$)`).exec(input.value);
    return m ? { start: m.index + m[1].length, end: m.index + m[0].length } : null;
  };
  const atEnd = () => ({ start: input.value.length, end: input.value.length });
  /** target: {trigger: {start,end}} (the typed `note:`), {name} (a note word in the input) or {} (append a new one). */
  const open = async (target) => {
    const valueNow = target.trigger ? input.value.slice(0, target.trigger.start) + input.value.slice(target.trigger.end) : input.value;
    const names = inputNames(valueNow);
    const isNew = !target.name;
    const name = isNew ? T.nextNoteName([...allNoteNames(), ...names]) : target.name;
    const others = new Set(state.lines.flatMap((l, j) => (j === state.editing ? [] : T.noteNames(T.parse(l)))).map((n) => n.toLowerCase()));
    noteDialog({
      name, names, isNew,
      text: isNew ? '' : await readNote(name),
      taken: (n) => (isNew || n.toLowerCase() !== name.toLowerCase()) && (others.has(n.toLowerCase()) || names.some((x) => x !== name && x.toLowerCase() === n.toLowerCase())),
      onSave: async (n, body) => { await writeNote(n, body); gitChanged(`${n}.txt`); },
      onPick: (v) => setTimeout(() => open(v ? { name: v } : {}), 0),
      onClose: (saved) => {
        if (saved) {
          const r = target.trigger || (target.name && wordRange(target.name)) || atEnd();
          put(r, `note:${saved}`);
        } else if (target.trigger) put(target.trigger, '');
        input.focus();
      },
    });
  };
  open({ trigger: { start, end } });
}

// ---------- derived data ----------

function visibleTasks() {
  const f = state.filter;
  const q = f.q.trim().toLowerCase();
  let items = [];
  state.lines.forEach((raw, i) => {
    const t = T.parse(raw);
    if (!state.showDone && t.done) return;
    if (f.project.length && !f.project.some((p) => t.projects.includes(p))) return;
    if (f.context.length && !f.context.some((c) => t.contexts.includes(c))) return;
    if (f.priority.length && !f.priority.includes(t.priority)) return;
    if (q && !raw.toLowerCase().includes(q)) return;
    items.push({ i, t });
  });

  return sortItems(items);
}

function sortItems(items) {
  const none = '￿';
  const cmp = {
    priority: (a, b) => (a.t.priority || none).localeCompare(b.t.priority || none),
    due: (a, b) => (a.t.tags.due || none).localeCompare(b.t.tags.due || none),
    created: (a, b) => (a.t.created || none).localeCompare(b.t.created || none),
    alpha: (a, b) => a.t.text.toLowerCase().localeCompare(b.t.text.toLowerCase()),
  }[state.sort];
  if (cmp) {
    items.sort((a, b) => (a.t.done - b.t.done) || cmp(a, b) || a.i - b.i);
  }
  return items;
}

// Saving writes the whole file in the current sort order (all tasks, ignoring filters).
function applySortToLines() {
  if (state.editing !== null) return;
  const sorted = sortItems(state.lines.map((raw, i) => ({ i, t: T.parse(raw) }))).map((x) => state.lines[x.i]);
  if (sorted.some((l, k) => l !== state.lines[k])) state.lines = sorted;
}

/**
 * Sidebar counts. Only tasks that match the other active sidebar filters count, so with a project
 * selected the contexts and priorities list just what exists in that project (`skip` = own section).
 */
function countBy(pick, includeDone = false, skip = null) {
  const f = state.filter;
  const m = new Map();
  for (const raw of state.lines) {
    const t = T.parse(raw);
    if (t.done && !includeDone) continue;
    if (skip !== 'project' && f.project.length && !f.project.some((p) => t.projects.includes(p))) continue;
    if (skip !== 'context' && f.context.length && !f.context.some((c) => t.contexts.includes(c))) continue;
    if (skip !== 'priority' && f.priority.length && !f.priority.includes(t.priority)) continue;
    for (const k of new Set(pick(t))) m.set(k, (m.get(k) || 0) + 1);
  }
  return [...m].sort((a, b) => a[0].localeCompare(b[0]));
}

// ---------- rendering ----------

const el = (tag, props = {}, ...kids) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...kids);
  return e;
};

/** Plain click: only this value (or none if it is the only one selected). Ctrl/Cmd/Shift click: add/remove it. */
function toggleFilter(key, value, multi = false) {
  const cur = state.filter[key];
  if (multi) state.filter[key] = cur.includes(value) ? cur.filter((v) => v !== value) : [...cur, value];
  else state.filter[key] = cur.length === 1 && cur[0] === value ? [] : [value];
  render();
}

function renderSidebar() {
  const side = $('#sidebar');
  side.replaceChildren();
  const section = (title, entries, key, label = (k) => k, cls = () => '') => {
    const h = el('h2', {}, el('span', { textContent: title }));
    if (state.filter[key].length) {
      h.append(el('button', {
        className: 'clear-x', textContent: '✕', title: `Clear ${title.toLowerCase()} filter`,
        onclick: () => { state.filter[key] = []; render(); },
      }));
    }
    side.append(h);
    if (!entries.length) side.append(el('div', { className: 'side-empty', textContent: 'None' }));
    for (const [k, n] of entries) {
      side.append(el('button', {
        className: 'side-item' + (state.filter[key].includes(k) ? ' active' : ''),
        onclick: (e) => toggleFilter(key, k, e.ctrlKey || e.metaKey || e.shiftKey),
      }, el('span', { className: cls(k), textContent: label(k) }), el('span', { className: 'n', textContent: n })));
    }
  };
  section('Projects', countBy((t) => t.projects, false, 'project'), 'project', (k) => '+' + k, () => 'tk-project');
  section('Contexts', countBy((t) => t.contexts, false, 'context'), 'context', (k) => '@' + k, () => 'tk-context');
  section('Priority', countBy((t) => (t.priority ? [t.priority] : []), false, 'priority'), 'priority', (k) => k, (k) => `tk-p${k}`);
}

/** Render a task description with clickable projects/contexts, tag badges and links. */
function renderText(t, i) {
  const frag = document.createDocumentFragment();
  // `inline code` (may contain spaces) is cut out first, everything else is handled word by word
  const segs = t.text.split(/(`[^`\n]+`)/);
  for (const seg of segs) {
    if (/^`[^`\n]+`$/.test(seg)) { frag.append(el('code', { className: 'code', textContent: seg.slice(1, -1) })); continue; }
    renderWords(frag, seg, t, i);
  }
  return frag;
}

function renderWords(frag, text, t, i) {
  for (const part of text.split(/(\s+)/)) {
    if (part.length > 1 && part[0] === '+') {
      frag.append(el('button', { className: 'chip project', textContent: part, onclick: (e) => toggleFilter('project', part.slice(1), e.ctrlKey || e.metaKey || e.shiftKey) }));
    } else if (part.length > 1 && part[0] === '@') {
      frag.append(el('button', { className: 'chip context', textContent: part, onclick: (e) => toggleFilter('context', part.slice(1), e.ctrlKey || e.metaKey || e.shiftKey) }));
    } else if (/^https?:\/\/\S+$/.test(part)) {
      frag.append(el('a', { href: part, textContent: part, target: '_blank', rel: 'noopener noreferrer' }));
    } else {
      const m = /^([^\s:]+):([^\s:]+)$/.exec(part);
      if (m && m[1] === 'note' && T.noteNames(t).includes(m[2])) {
        frag.append(el('button', { className: 'chip note', textContent: part, title: 'Open note', onclick: () => editNote(i, m[2]) }));
      } else if (m && !m[2].startsWith('//')) {
        let cls = 'meta';
        if (m[1] === 'due' && !t.done) cls += ' ' + (T.dueStatus(t) || '');
        frag.append(el('span', { className: cls.trim(), textContent: part }));
      } else {
        frag.append(part);
      }
    }
  }
}

function renderTask({ i, t }, idx) {
  const ds = t.done ? null : T.dueStatus(t);
  const li = el('li', { className: 'task' + (t.done ? ' done' : '') + (ds && ds !== 'upcoming' ? ` due-${ds}` : '') });
  li.dataset.idx = idx;

  // Drag handle: reorder / change priority when sorted by priority.
  const canDrag = state.sort === 'priority' && !t.done && state.editing !== i;
  const grip = el('span', { className: 'grip' + (canDrag ? '' : ' off'), title: canDrag ? 'Drag to change priority' : '' },
    el('i', { className: 'fa-solid fa-grip-vertical' }));
  grip.draggable = canDrag;
  grip.ondragstart = (e) => {
    dragLine = i;
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', String(i));
    e.dataTransfer.setDragImage(li, 12, 12);
    li.classList.add('dragging');
  };
  grip.ondragend = () => { dragLine = null; clearDropMarks(); li.classList.remove('dragging'); };

  const cb = el('input', { type: 'checkbox', checked: t.done, title: t.done ? 'Mark as open' : 'Mark as done' });
  cb.setAttribute('aria-label', 'Completed');
  cb.onchange = () => replaceLine(i, t.done ? T.reopen(t.raw) : T.complete(t.raw));

  // Priority picker: click opens the same popup as the priority button of the touch toolbar (no brackets).
  const priBtn = el('button', {
    type: 'button', className: 'dd-btn pri-btn', disabled: t.done, title: 'Priority',
  }, t.priority ? t.priority : '—');
  priBtn.dataset.p = t.priority || '';
  priBtn.setAttribute('aria-haspopup', 'true');
  priBtn.setAttribute('aria-label', 'Priority');
  priBtn.onclick = () => togglePrioPopup(priBtn, t.priority || null, (L) => replaceLine(i, T.setPriority(t.raw, L)));
  const pri = el('div', { className: 'dd pri-dd' }, priBtn);

  const body = el('div', { className: 'body', title: 'Double-click to edit' });
  if (state.editing === i) {
    const input = el('input', { type: 'text', className: 'edit', value: t.raw });
    let finished = false;
    const finish = (save) => {
      if (finished) return;
      finished = true;
      state.editing = null;
      const v = T.normalizeDue(T.fixPriorityCase(input.value.trim()));
      if (!save || v === t.raw) return render();
      if (v === '') removeLine(i); else replaceLine(i, v);
    };
    const helper = buildEditHelper(input, t);
    const upd = () => helper.update();
    input.addEventListener('input', upd);
    input.addEventListener('click', upd);
    input.addEventListener('keyup', (e) => { if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) upd(); });
    input.onkeydown = (e) => {
      const cleared = deleteWord(e, input);   // Ctrl+Backspace/Delete: remove whole word
      if (cleared) {
        if (cleared === 'project') helper.showList('tag');
        else if (cleared === 'context') helper.showList('ctx');
        return;
      }
      if (e.key === 'Enter') finish(true);
      else if (e.key === 'Escape') {
        if (!helper.closePopup()) finish(false);   // first Esc closes an open helper popup
      }
    };
    input.onblur = () => { if (!document.getElementById('noteDlg')) finish(true); };
    const field = attachHighlight(input);
    attachNoteTrigger(input, noteForInput);
    helper.sync = field.sync;
    body.append(field.el, helper);
    queueMicrotask(() => input.focus());
  } else {
    body.append(renderText(t, i));
    const dates = [t.created && `created ${t.created}`, t.completed && `done ${t.completed}`].filter(Boolean);
    if (dates.length && state.showDates) body.append(el('span', { className: 'dates', textContent: dates.join(' · ') }));
    else if (dates.length) {   // hidden: tooltip + status bar while hovering
      body.title = dates.join(' · ');
      li.addEventListener('mouseenter', () => { if (statusHover) statusHover.textContent = dates.join(' · '); });
      li.addEventListener('mouseleave', () => { if (statusHover) statusHover.textContent = ''; });
    }
    body.ondblclick = (e) => {
      if (e.target.closest('button, a')) return;
      state.editing = i;
      render();
    };
  }

  const noteList = T.noteNames(t);
  const hasNote = noteList.length > 0;
  let noteCtl;
  if (noteList.length > 1) {
    // several notes: the button opens a dropdown of all of them
    const nb = el('button', { type: 'button', className: 'dd-btn note-btn on', title: `${noteList.length} notes`, 'aria-label': 'Notes' },
      el('i', { className: 'fa-solid fa-note-sticky' }), el('small', { className: 'note-count', textContent: String(noteList.length) }));
    nb.setAttribute('aria-haspopup', 'true');
    nb.setAttribute('aria-expanded', 'false');
    const nm = el('div', { className: 'dd-menu note-menu', hidden: true, role: 'menu' },
      ...noteList.map((n) => el('button', { type: 'button', role: 'menuitem', onclick: () => editNote(i, n) }, el('i', { className: 'fa-solid fa-note-sticky' }), ` ${n}`)),
      el('hr'), el('button', { type: 'button', role: 'menuitem', onclick: () => editNote(i, null) }, el('i', { className: 'fa-solid fa-plus' }), ' New note'));
    noteCtl = el('div', { className: 'dd note-dd' }, nb, nm);
    wireDropdown(noteCtl);
  } else {
    noteCtl = el('button', { className: 'note-btn' + (hasNote ? ' on' : ''), title: hasNote ? 'Note' : 'Add note', onclick: () => editNote(i) },
      el('i', { className: 'fa-solid fa-note-sticky' }));
  }
  const actions = el('div', { className: 'actions' + (hasNote ? ' has-note' : '') },
    noteCtl,
    el('button', { title: 'Edit', onclick: () => { state.editing = i; render(); } }, el('i', { className: 'fa-solid fa-pen' })),
    el('button', { className: 'del', title: 'Delete', onclick: () => removeLine(i) }, el('i', { className: 'fa-solid fa-xmark' })));
  if (noteList.length <= 1) actions.children[0].setAttribute('aria-label', 'Note');
  actions.children[1].setAttribute('aria-label', 'Edit');
  actions.lastChild.setAttribute('aria-label', 'Delete');

  if (isTauri && state.lineNumbers) {
    // the line number takes the place of the drag handle (the handle shows on hover)
    li.append(el('span', { className: 'slot' }, grip, el('span', { className: 'lineno', title: `Line ${i + 1}` }, String(i + 1))), cb, pri, body, actions);
  } else li.append(grip, cb, pri, body, actions);
  return li;
}

function render() {
  $('#fileName').textContent = state.fileName;
  {
    const dav = isTauri && isRemote(state.path);
    const kind = dav ? ['cloud', 'WebDAV'] : (isTauri && state.git && state.git.repo ? ['code-branch', 'Git folder'] : ['hard-drive', 'Local file']);
    const k = $('#fileKind');
    k.className = `fa-solid fa-${kind[0]}`;
    k.title = kind[1];
  }
  renderFileList();
  $('#dirty').hidden = !state.dirty;
  $('#undo').disabled = !state.undo.length;
  document.querySelectorAll('[data-sort]').forEach((b) => b.setAttribute('aria-checked', String(b.dataset.sort === state.sort)));
  $('#showDone').setAttribute('aria-checked', state.showDone);
  $('#calToggle').setAttribute('aria-checked', state.calendar);
  $('#minimalToggle').setAttribute('aria-checked', state.minimal);
  $('#statusToggle').setAttribute('aria-checked', state.statusBar);
  $('#touchToggle').setAttribute('aria-checked', state.touchBar);
  $('#datesToggle').setAttribute('aria-checked', state.showDates);
  $('#touchBar').hidden = !state.touchBar || state.view === 'text';
  document.body.classList.toggle('minimal', state.minimal);
  document.body.classList.toggle('nostatus', !state.statusBar);
  if (!state.minimal) document.body.classList.remove('show-bar');
  applyTheme();
  $('#openFolder').style.display = isTauri || hasDirPicker ? '' : 'none';
  $('#openDav').style.display = isTauri ? '' : 'none';
  $('#davTab').style.display = isTauri ? '' : 'none';
  renderGit();
  $('#autoSaveToggle').style.display = isTauri ? '' : 'none';
  $('#autoSaveToggle').setAttribute('aria-checked', state.autoSave);
  $('#lineNoToggle').style.display = isTauri ? '' : 'none';
  $('#lineNoToggle').setAttribute('aria-checked', state.lineNumbers);
  $('#framelessToggle').style.display = isTauri ? '' : 'none';
  $('#framelessToggle').setAttribute('aria-checked', state.frameless);
  $('#winCtl').hidden = !(isTauri && state.frameless);
  applyWindowFx();
  if (isTauri && appliedFrameless !== state.frameless) {
    appliedFrameless = state.frameless;
    try { tauri.window.getCurrentWindow().setDecorations(!state.frameless).catch(() => {}); } catch { /* ignore */ }
  }
  $('#archive').disabled = !state.lines.some((l) => T.parse(l).done);
  const vt = $('#viewToggle');
  vt.replaceChildren(el('i', { className: 'fa-solid ' + (state.view === 'list' ? 'fa-file-lines' : 'fa-list') }),
    state.view === 'list' ? ' Text view' : ' List view');

  const listMode = state.view === 'list';
  $('#list').hidden = !listMode;
  $('#raw').hidden = listMode;
  const showSide = listMode && state.sidebar;
  $('#sidebar').hidden = !showSide;
  $('#layout').classList.toggle('no-side', !showSide);
  document.body.classList.toggle('side-open', isAndroid && showSide);
  $('#scrim').hidden = !(isAndroid && showSide);
  $('#sideToggle').setAttribute('aria-checked', state.sidebar);
  $('#sideToggle').disabled = !listMode;
  $('#status').hidden = !listMode;
  document.title = state.fileName + (state.dirty ? '*' : '') + ' – todo.txt' + (appVersion ? `  v${appVersion}` : '');
  if (isTauri && tauri.window) {
    try { tauri.window.getCurrentWindow().setTitle(document.title).catch(() => {}); } catch { /* ignore */ }
  }

  if (!listMode) {
    const ta = $('#raw');
    if (document.activeElement !== ta) ta.value = state.lines.join('\n');
    return;
  }

  renderSidebar();
  const items = visibleTasks();
  const list = $('#list');
  viewItems = items;
  list.replaceChildren(...items.map((it, k) => renderTask(it, k)));

  const open = state.lines.filter((l) => !T.parse(l).done).length;
  const f = state.filter;
  const active = [f.q.trim() && `"${f.q.trim()}"`, ...f.project.map((p) => '+' + p), ...f.context.map((c) => '@' + c), ...f.priority.map((p) => `(${p})`)].filter(Boolean);
  const status = $('#status');
  const right = el('span', { className: 'status-right' }, `${open} open · ${state.lines.length} total · showing ${items.length}${state.gitMsg ? ' · ' + state.gitMsg : ''}`);
  if (active.length) {
    right.append(' · filter: ' + active.join(' '), ' ',
      el('button', { textContent: 'clear', onclick: () => { state.filter.project = []; state.filter.context = []; state.filter.priority = []; composer.clearSearch(); render(); } }));
  }
  status.replaceChildren();
  const gs = state.gitStatus;
  if (state.path && isRemote(state.path) && state.davState?.offline) {
    const p = state.davState.pending;
    status.append(el('span', { className: 'status-offline', title: 'The WebDAV server is not reachable. Changes are kept in a local cache and synced automatically when it is back.',
      textContent: 'offline' + (p ? ` · ${p[0] + p[1]} pending` : '') }));
  }
  if (state.path && isRemote(state.path)) {
    let full = state.path;
    try { full = decodeURI(state.path); } catch { /* keep raw */ }
    // "server - folder/file": host of the server and the path below the server's base URL
    let shown = full;
    try {
      const srv = serverFor(davServers, state.path);
      const host = new URL(srv ? srv.url : state.path).hostname;
      const base = srv ? decodeURI(srv.url) : `${new URL(state.path).origin}/`;
      shown = `${host} - ${full.startsWith(base) ? full.slice(base.length) : decodeURI(new URL(state.path).pathname).replace(/^\/+/, '')}`;
    } catch { /* keep the full address */ }
    status.append(el('span', { className: 'status-folder status-dav', title: full, textContent: shown }));
  } else if (state.folder) {
    const base = state.folder.replace(/[\/]+$/, '').split(/[\/]/).pop() || state.folder;
    status.append(el('span', { className: 'status-folder', title: state.folder, textContent: base }));
  }
  if (gs) {
    status.append(el('span', { className: 'status-git' + (gs.changed ? ' dirty' : ' clean'), title: gs.summary ? `${gs.commit}: ${gs.summary}` : 'git', textContent: 'git: ' + gs.label }));
  }
  statusHover = el('span', { className: 'status-hover' });
  status.append(statusHover, right);
  if (!items.length) {
    list.replaceChildren(el('li', { className: 'empty', textContent: state.lines.length ? 'No tasks match.' : 'No tasks yet. Add one above, or open a todo.txt file.' }));
  }
}

// ---------- edit helper (clickable tag / context / priority / due) ----------

function buildEditHelper(input, t) {
  const bar = el('div', { className: 'edit-helper' });
  const pop = el('div', { className: 'ac helper-pop', hidden: true });
  let openKind = null;
  let calMonth = null;
  bar.onmousedown = (e) => e.preventDefault();   // keep the caret/focus in the text field

  const close = () => { pop.hidden = true; pop.replaceChildren(); openKind = null; };
  bar.closePopup = () => { const was = !pop.hidden; close(); return was; };
  const open = (kind, content, cls) => {
    openKind = kind;
    pop.className = `ac helper-pop ${cls}`;
    pop.replaceChildren(content);
    pop.hidden = false;
  };
  const refresh = () => input.dispatchEvent(new Event('input'));
  const toEnd = () => { input.setSelectionRange(input.value.length, input.value.length); input.focus(); refresh(); };

  const insertWord = (w) => {
    checkpoint(input);
    const v = input.value;
    const s = input.selectionStart ?? v.length;
    const e = input.selectionEnd ?? s;
    let before = v.slice(0, s);
    const after = v.slice(e);
    const partial = /(?:^|\s)([+@]\S*)$/.exec(before);
    if (partial && partial[1][0] === w[0]) before = before.slice(0, before.length - partial[1].length);
    const text = (before && !/\s$/.test(before) ? ' ' : '') + w + (/^\s/.test(after) ? '' : ' ');
    input.value = before + text + after;
    const c = (before + text).length;
    input.setSelectionRange(c, c);
    input.focus();
    refresh();
  };

  const list = (rows, empty) => {
    const frag = document.createDocumentFragment();
    if (!rows.length) frag.append(el('div', { className: 'side-empty', textContent: empty }));
    for (const r of rows) {
      frag.append(el('button', { type: 'button', className: 'ac-item', onclick: () => { r.pick(); close(); } },
        el('span', { className: r.cls || '', textContent: r.label }),
        el('span', { className: 'n', textContent: r.count || '' })));
    }
    return frag;
  };

  const setDue = (iso) => {
    checkpoint(input);
    const v = input.value;
    input.value = /(^|\s)due:\S*/.test(v)
      ? v.replace(/(^|\s)due:\S*/, `$1due:${iso}`)
      : `${v.replace(/\s+$/, '')} due:${iso}`;
    toEnd();
  };
  const dueDate = () => parseIsoDate(T.parse(input.value).tags.due);
  const drawCal = () => open('due', buildCalendar({
    month: calMonth,
    selected: dueDate(),
    onPick: (d) => { setDue(T.isoDate(d)); close(); },
    onMonth: (first) => { calMonth = first; drawCal(); },
  }), 'cal');

  // Typing `due`, `due:...` or a bare date opens the calendar, like in the add bar.
  bar.update = () => {
    if (!state.calendar) return;
    const { start, word } = wordAt(input);
    const hit = dateAssist(input.value, start, word);
    if (!hit) { if (openKind === 'auto') close(); return; }
    if (openKind !== 'auto') calMonth = null;
    const base = calMonth || new Date();
    calMonth = new Date(hit.year ?? base.getFullYear(), hit.month ?? base.getMonth(), 1);
    const draw = () => open('auto', buildCalendar({
      month: calMonth,
      selected: hit.selected,
      onPick: (d) => { const w = wordAt(input); replaceWordAt(input, w.start, w.end, `due:${T.isoDate(d)}`); },
      onMonth: (first) => { calMonth = first; draw(); },
    }), 'cal');
    draw();
  };

  bar.showList = (kind) => {
    const isTag = kind === 'tag';
    const rows = countBy((x) => (isTag ? x.projects : x.contexts), true).map(([n, c]) => {
      const word = (isTag ? '+' : '@') + n;
      return { label: word, count: c, cls: isTag ? 'tk-project' : 'tk-context', pick: () => insertWord(word) };
    });
    open(kind, list(rows, isTag ? 'No tags yet. Type +name' : 'No contexts yet. Type @name'), 'list');
  };

  const button = (label, title, onclick) => el('button', { type: 'button', title, onclick },
    ...(Array.isArray(label) ? label : [label]));

  if (!t.done) {
    bar.append(button('(A)', 'Set priority', () => {
      if (openKind === 'pri') return close();
      const used = new Map(countBy((x) => (x.priority ? [x.priority] : [])));
      const setPri = (L) => () => { checkpoint(input); input.value = T.setPriority(input.value, L); toEnd(); };
      const rows = [{ label: 'None', pick: setPri(null) }];
      for (let c = 65; c <= 90; c++) {
        const L = String.fromCharCode(c);
        rows.push({ label: `(${L})`, cls: `tk-p${L}`, count: used.get(L), pick: setPri(L) });
      }
      open('pri', list(rows), 'list');
    }));
  }
  bar.append(
    button('+tag', 'Add a tag (project)', () => (openKind === 'tag' ? close() : bar.showList('tag'))),
    button('@context', 'Add a context', () => (openKind === 'ctx' ? close() : bar.showList('ctx'))),
    button([el('i', { className: 'fa-solid fa-calendar-days' }), ' due'], 'Set due date', () => {
      if (openKind === 'due') return close();
      const d = dueDate() || new Date();
      calMonth = new Date(d.getFullYear(), d.getMonth(), 1);
      drawCal();
    }),
    pop);
  return bar;
}

// ---------- drag & drop (priority sort): drop between tasks to adopt that priority ----------

let dragLine = null;     // line index being dragged
let viewItems = [];      // [{i, t}] currently displayed, in order

function clearDropMarks() {
  document.querySelectorAll('.drop-before, .drop-after').forEach((e) => e.classList.remove('drop-before', 'drop-after'));
}

function dropPoint(e) {
  const li = e.target.closest('.task');
  if (!li || li.dataset.idx === undefined) return null;
  const r = li.getBoundingClientRect();
  const after = e.clientY > r.top + r.height / 2;
  return { li, after, pos: +li.dataset.idx + (after ? 1 : 0) };
}

/** Move `lineIdx` to display position `pos`; it takes the priority of the open task above it (or below, at the top). */
function dropTask(lineIdx, pos) {
  const isOpen = (k) => viewItems[k] && viewItems[k].i !== lineIdx && !viewItems[k].t.done;
  let prev = null;
  let next = null;
  for (let k = pos - 1; k >= 0; k--) if (isOpen(k)) { prev = viewItems[k]; break; }
  for (let k = pos; k < viewItems.length; k++) if (isOpen(k)) { next = viewItems[k]; break; }

  const orig = state.lines[lineIdx];
  const source = prev || next;
  const raw = source ? T.setPriority(orig, source.t.priority) : orig;

  const rest = state.lines.map((l, i) => ({ l, i })).filter((o) => o.i !== lineIdx);
  let at = rest.length;
  if (prev) at = rest.findIndex((o) => o.i === prev.i) + 1;
  else if (next) at = rest.findIndex((o) => o.i === next.i);
  rest.splice(at, 0, { l: raw, i: lineIdx });
  const lines = rest.map((o) => o.l);
  if (lines.every((l, k) => l === state.lines[k])) return;   // nothing changed
  commit(lines);
}

const taskList = $('#list');
taskList.addEventListener('dragover', (e) => {
  if (dragLine === null) return;
  const d = dropPoint(e);
  if (!d) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  clearDropMarks();
  d.li.classList.add(d.after ? 'drop-after' : 'drop-before');
});
taskList.addEventListener('dragleave', (e) => {
  if (!taskList.contains(e.relatedTarget)) clearDropMarks();
});
taskList.addEventListener('drop', (e) => {
  if (dragLine === null) return;
  e.preventDefault();
  const d = dropPoint(e);
  const line = dragLine;
  dragLine = null;
  clearDropMarks();
  if (d) dropTask(line, d.pos);
});

// ---------- events ----------

const composer = createComposer({
  form: $('#addForm'),
  input: $('#addInput'),
  hl: $('#hl'),
  ac: $('#ac'),
  modeBtn: $('#modeBtn'),
  getWords: (kind) => (kind === 'priority'
    ? countBy((t) => (t.priority ? [t.priority] : []))
    : countBy((t) => (kind === 'project' ? t.projects : t.contexts), true)),
  onAdd: (text) => commit([...state.lines, T.withCreationDate(T.normalizeDue(T.fixPriorityCase(text)))]),
  calendarEnabled: () => state.calendar,
  onNote: (inp) => noteForInput(inp),
  onSearch: (q) => { state.filter.q = q; render(); },
});

// ---------- priority popup (touch toolbar button and the priority button of each task) ----------
let prioPop = null;
let prioAnchor = null;
function closePrioPopup() {
  if (!prioPop) return;
  prioPop.remove();
  prioPop = null;
  prioAnchor = null;
  document.removeEventListener('mousedown', prioOutside, true);
  document.removeEventListener('keydown', prioKey, true);
}
function prioOutside(e) { if (prioPop && !prioPop.contains(e.target) && !prioAnchor.contains(e.target)) closePrioPopup(); }
function prioKey(e) { if (e.key === 'Escape') { e.stopPropagation(); closePrioPopup(); } }
/** Popup with A-D and "none" (plus the current letter if it is outside A-D) above/below `anchor`; `pick(letter|null)`. */
function togglePrioPopup(anchor, current, pick) {
  const same = prioAnchor === anchor;
  closePrioPopup();
  if (same) return;
  const mk = (label, title, cls, fn) => {
    const b = el('button', { type: 'button', className: `tb ${cls}`, title, onclick: fn });
    b.append(label);
    b.setAttribute('aria-label', title);
    return b;
  };
  const letters = ['A', 'B', 'C', 'D'];
  if (current && !letters.includes(current)) letters.push(current);
  prioAnchor = anchor;
  prioPop = el('div', { className: 'tb-pop', role: 'menu', 'aria-label': 'Priority' },
    ...letters.map((L) => mk(L, `Priority ${L}`, `tb-p tk-p${L}`, () => { closePrioPopup(); pick(L); })),
    mk(el('i', { className: 'fa-solid fa-ban' }), 'No priority', 'tb-p', () => { closePrioPopup(); pick(null); }));
  prioPop.addEventListener('mousedown', (e) => { if (e.target.closest('button')) e.preventDefault(); });
  document.body.append(prioPop);
  const r = anchor.getBoundingClientRect();
  const pr = prioPop.getBoundingClientRect();
  prioPop.style.left = `${Math.max(4, Math.min(r.left, innerWidth - pr.width - 4))}px`;
  prioPop.style.top = `${r.top - pr.height - 6 >= 4 ? r.top - pr.height - 6 : r.bottom + 6}px`;
  document.addEventListener('mousedown', prioOutside, true);
  document.addEventListener('keydown', prioKey, true);
}

// ---------- touch toolbar: buttons under the input that edit the text being typed (priority, +project, @context, due, note) ----------
{
  const input = $('#addInput');
  const bar = $('#touchBar');
  /** Insert `text` at the caret (a space before it if needed), keep the focus and let the composer react (popups). */
  const insert = (text) => {
    input.focus();
    checkpoint(input);
    const v = input.value;
    const a = input.selectionStart ?? v.length;
    const b = input.selectionEnd ?? a;
    const sep = a > 0 && !/\s$/.test(v.slice(0, a)) ? ' ' : '';
    input.value = v.slice(0, a) + sep + text + v.slice(b);
    const caret = a + sep.length + text.length;
    input.setSelectionRange(caret, caret);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  /** Set (letter) or remove (null) the `(X) ` priority at the start of the input. */
  const setPrio = (L) => {
    if (composer.mode !== 'add') composer.setMode('add');
    input.focus();
    checkpoint(input);
    const rest = input.value.replace(/^\([A-Za-z]?\)?\s*/, '');
    const pre = L ? `(${L}) ` : '';
    const caret = Math.max(0, (input.selectionStart ?? 0) - (input.value.length - rest.length)) + pre.length;
    input.value = pre + rest;
    input.setSelectionRange(caret, caret);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const btn =(label, title, onclick, cls = '') => {
    const b = el('button', { type: 'button', className: `tb ${cls}`.trim(), title, onclick });
    b.append(...(Array.isArray(label) ? label : [label]));
    b.setAttribute('aria-label', title);
    return b;
  };
  const ic = (n) => el('i', { className: `fa-solid fa-${n}` });
  bar.append(
    btn([ic('arrow-up-wide-short')], 'Priority', (e) => togglePrioPopup(e.currentTarget, null, setPrio), 'tb-w tb-prio'),
    btn([ic('folder-tree'), ' +'], 'Project (+name)', () => insert('+'), 'tb-w'),
    btn([ic('at'), ''], 'Context (@name)', () => insert('@'), 'tb-w'),
    btn([ic('calendar-day')], 'Due date (opens the calendar)', () => insert('due:'), 'tb-w'),
    btn([ic('note-sticky')], 'Add a note', () => { insert('note:'); noteForInput(input); }, 'tb-w'),
    el('span', { className: 'spacer' }),
    el('button', { type: 'submit', className: 'tb tb-add', title: 'Add task (Enter)', 'aria-label': 'Add task' }, ic('plus'), ' Add'));
  // keep the focus (and the on-screen keyboard) in the input when tapping a button
  bar.addEventListener('mousedown', (e) => { if (e.target.closest('button')) e.preventDefault(); });
}

function buildThemeMenu() {
  const menu = $('#themeMenu');
  const keepScroll = menu.scrollTop;
  menu.replaceChildren();
  const label = (key) => key.replace(/^user:/, '') === 'grand-theft-six' ? 'Grand Theft Six' : key.replace(/^user:/, '').replace(/^./, (c) => c.toUpperCase());
  const cur = themeKey(state.theme);
  const vars = cur ? themeVariants(cur) : [];
  if (vars.length > 1) {   // variant dropdown of the chosen theme
    const sel = el('select', { id: 'themeVariant', title: 'Variant of this theme' });
    for (const [l, v] of vars) sel.append(el('option', { value: v, textContent: l, selected: v === state.theme }));
    sel.onchange = () => { state.theme = sel.value; persist(); render(); };
    menu.append(el('label', { className: 'theme-variant' }, 'Variant ', sel));
  }
  const add = (key, text) => {
    const b = el('button', { role: 'menuitemradio' }, el('span', { className: 'tick' }, el('i', { className: 'fa-solid fa-check' })), ' ' + text);
    b.dataset.theme = key;
    b.onclick = () => {
      const v = key ? themeVariants(key) : [];
      state.theme = !key ? '' : !v.length ? key : (v.find(([, f]) => f === state.theme) || (v.find(([l]) => l === (themeVariants(cur).find(([, f]) => f === state.theme) || [])[0]) || v[0]))[1];
      persist(); render(); buildThemeMenu();
    };
    menu.append(b);
  };
  const keys = [...new Set(['', ...THEMES.map(themeKey)])];
  for (const key of keys) add(key, key ? label(key) : 'Default (system light/dark)');
  const own = [...new Set(Object.keys(userThemes).sort().map((n) => themeKey('user:' + n)))];
  if (own.length) {
    menu.append(el('hr'), el('small', { className: 'set-hint', textContent: 'Themes (bundled themes folder, ~/.todo.txt/themes overrides)' }));
    for (const k of own) add(k, label(k));
  } else if (isTauri) {
    menu.append(el('hr'), el('small', { className: 'set-hint', textContent: 'No themes found. Put NAME.css files in a themes folder next to the program or in ~/.todo.txt/themes (NAME-light.css / NAME-dark.css give a variant dropdown).' }));
  }
  applyTheme();
  menu.scrollTop = keepScroll;
}
/** (Re)read the own themes (desktop); called at start and whenever the settings open. */
async function loadUserThemes() {
  if (!isTauri) return;
  try {
    const list = await invoke('list_user_themes');
    const next = Object.fromEntries(list.map(([n, css]) => [n, css]));
    if (JSON.stringify(next) === JSON.stringify(userThemes)) return;
    userThemes = next;
    buildThemeMenu();
  } catch { /* no themes folder */ }
}
/** Re-read the chosen theme from disk/server (bundled themes folder and ~/.todo.txt/themes). */
async function reloadTheme() {
  if (isTauri) {
    try { userThemes = Object.fromEntries((await invoke('list_user_themes')).map(([n, css]) => [n, css])); } catch { /* no themes folder */ }
    const st = document.getElementById('themeStyle');
    if (st) st.remove();   // force re-injection
  }
  buildThemeMenu();
  applyTheme();
}
$('#themeReload').onclick = reloadTheme;
buildThemeMenu();
loadUserThemes();
// No browser context menu (right click) outside the input fields.
document.addEventListener('contextmenu', (e) => { if (!e.target.closest('input, textarea')) e.preventDefault(); });
// Settings overlay (gear): tabs, scrolls when the window is small.
const settingsEl = $('#settings');
function showTab(name) {
  if (name === 'webdav') dav.renderPane(settingsEl.querySelector('[data-pane=webdav]'));
  settingsEl.querySelectorAll('[data-tab]').forEach((t) => t.setAttribute('aria-selected', String(t.dataset.tab === name)));
  settingsEl.querySelectorAll('[data-pane]').forEach((p) => { p.hidden = p.dataset.pane !== name; });
}
settingsEl.querySelectorAll('[data-tab]').forEach((t) => { t.onclick = () => showTab(t.dataset.tab); });
const closeSettings = () => { settingsEl.hidden = true; };
$('#optBtn').onclick = (e) => { e.stopPropagation(); closeMenus(); settingsEl.hidden = false; loadUserThemes(); };
$('#setClose').onclick = closeSettings;
settingsEl.onclick = (e) => { if (e.target === settingsEl) closeSettings(); };
document.querySelectorAll('[data-sort]').forEach((b) => {
  b.onclick = () => { state.sort = b.dataset.sort; persist(); render(); };
});
$('#showDone').onclick = () => { state.showDone = !state.showDone; persist(); render(); };
$('#viewToggle').onclick = () => { state.view = state.view === 'list' ? 'text' : 'list'; render(); };
$('#statusToggle').onclick = () => { state.statusBar = !state.statusBar; if (isTauri) setCfg('statusbar', state.statusBar); persist(); render(); };
$('#minimalToggle').onclick = () => { state.minimal = !state.minimal; persist(); render(); };
$('#datesToggle').onclick = () => { state.showDates = !state.showDates; if (isTauri) setCfg('showdates', state.showDates); persist(); render(); };
$('#touchToggle').onclick = () => { state.touchBar = !state.touchBar; if (isTauri) setCfg('touchbar', state.touchBar); persist(); render(); };
$('#calToggle').onclick = () => { state.calendar = !state.calendar; persist(); render(); };
$('#autoSaveToggle').onclick = () => {
  state.autoSave = !state.autoSave;
  setCfg('autosave', state.autoSave);
  render();
  scheduleAutoSave();
};
let appliedFrameless = null;
let appliedOnTop = null;

// Window effects (desktop): always on top.
function applyWindowFx() {
  $('#onTopToggle').style.display = isTauri ? '' : 'none';
  $('#onTopToggle').setAttribute('aria-checked', state.onTop);
  if (!isTauri) return;
  if (appliedOnTop !== state.onTop) {
    appliedOnTop = state.onTop;
    try { tauri.window.getCurrentWindow().setAlwaysOnTop(state.onTop).catch(() => {}); } catch { /* ignore */ }
  }
}
function toggleOnTop() { if (!isTauri) return; state.onTop = !state.onTop; setCfg('ontop', state.onTop); render(); }
$('#onTopToggle').onclick = toggleOnTop;
function toggleFrameless() {
  if (!isTauri) return;
  state.frameless = !state.frameless;
  setCfg('frameless', state.frameless);
  render();
}
$('#framelessToggle').onclick = toggleFrameless;
const win = () => tauri.window.getCurrentWindow();
$('#winMin').onclick = () => win().minimize();
$('#winClose').onclick = () => win().close();
// Frameless: drag the window by any empty area (top bar gaps, space around the entries); double click = maximize.
document.addEventListener('mousedown', (e) => {
  if (!isTauri || !state.frameless || e.button !== 0) return;
  const t = e.target;
  const free = t === document.documentElement || t === document.body || t.id === 'layout' || t.tagName === 'MAIN'
    || t.id === 'addForm' || t.classList.contains('topbar') || t.classList.contains('spacer') || t.classList.contains('win-ctl')
    || t.id === 'sidebar' || t.classList.contains('empty');
  if (!free) return;
  if (e.detail === 2) win().toggleMaximize(); else win().startDragging();
});
// Minimal mode: the top bar slides in as an overlay when the mouse is above the first entry.
{
  const bar = document.querySelector('.topbar');
  document.addEventListener('mousemove', (e) => {
    if (!state.minimal) return;
    const shown = document.body.classList.contains('show-bar');
    if (!shown) {
      const first = document.querySelector('#list .task, #raw:not([hidden])');
      const limit = Math.min(Math.max(first ? first.getBoundingClientRect().top : 16, 12), 40);
      if (e.clientY < limit) document.body.classList.add('show-bar');
    } else if (e.clientY > bar.offsetHeight + 8 && !bar.querySelector('.dd-menu:not([hidden])') && !bar.contains(document.activeElement)) {
      document.body.classList.remove('show-bar');
    }
  });
}
$('#lineNoToggle').onclick = () => {
  state.lineNumbers = !state.lineNumbers;
  setCfg('linenumbers', state.lineNumbers);
  render();
};
$('#sideToggle').onclick = () => { state.sidebar = !state.sidebar; persist(); render(); };
// Dropdown menus (File, View): one open at a time.
const closeMenus = () => document.querySelectorAll('.dd').forEach((d) => {
  const m = d.querySelector('.dd-menu');
  if (!m) return;   // e.g. the priority button (popup, not a dropdown)
  m.hidden = true;
  d.querySelector('.dd-btn').setAttribute('aria-expanded', 'false');
});

// Keep an open menu inside the window: shift it left if it would stick out on the
// right, cap its width/height to the viewport and scroll inside it when too tall.
function fitMenu(menu) {
  const m = 8, vw = document.documentElement.clientWidth, vh = window.innerHeight;
  menu.style.left = '0px';
  menu.style.maxWidth = (vw - 2 * m) + 'px';
  menu.style.maxHeight = '';
  menu.style.overflowY = 'auto';
  let r = menu.getBoundingClientRect();
  let shift = 0;
  if (r.right > vw - m) shift = vw - m - r.right;
  if (r.left + shift < m) shift = m - r.left;
  menu.style.left = shift + 'px';
  menu.style.maxHeight = Math.max(120, vh - r.top - m) + 'px';
}
window.addEventListener('resize', () => closeMenus());

function wireDropdown(d) {
  const btn = d.querySelector('.dd-btn');
  const menu = d.querySelector('.dd-menu');
  btn.onclick = (e) => {
    e.stopPropagation();
    const open = menu.hidden;
    closeMenus();
    menu.hidden = !open;
    btn.setAttribute('aria-expanded', String(open));
    if (open) fitMenu(menu);
  };
  // Re-fit when the content changes (e.g. the theme submenu expands).
  new MutationObserver(() => { if (!menu.hidden) fitMenu(menu); })
    .observe(menu, { childList: true, subtree: true, attributes: true, attributeFilter: ['hidden'] });
  menu.onclick = (e) => {
    e.stopPropagation();
    const b = e.target.closest('button');
    if (b && !('keep' in b.dataset)) closeMenus();
  };
}
document.querySelectorAll('.dd').forEach(wireDropdown);
document.addEventListener('click', closeMenus);
// Also on mousedown anywhere outside an open menu (capture: works even where click is swallowed,
// e.g. window dragging in frameless mode or handlers that stop propagation).
document.addEventListener('mousedown', (e) => {
  if (e.target.closest && e.target.closest('.dd')) return;
  closeMenus();
}, true);

$('#undo').onclick = undo;
$('#openFolder').onclick = openFolder;
$('#openDav').onclick = () => dav.openBrowser();
$('#open').onclick = isAndroid ? () => openFiles(androidDeps()) : openFile;
$('#new').onclick = isAndroid ? () => openFiles(androidDeps(), { focusNew: true }) : () => { if (confirmDiscard()) { leaveFolder(); loadText('', 'todo.txt'); } };
$('#save').onclick = () => save();
$('#saveAs').onclick = () => save({ as: true });
$('#keepFile').style.display = isTauri ? '' : 'none';
$('#keepFile').onclick = async () => {
  if (!state.path) return alert('Save the list to a file first.');
  await rememberFile(state.path, true);
  renderFileList();
};
$('#archive').onclick = archiveDone;
$('#refresh').onclick = () => checkExternalChange(true);
$('#fileInput').onchange = (e) => {
  const f = e.target.files[0];
  e.target.value = '';
  if (f) openFromFileObject(f);
};

$('#raw').onchange = (e) => {
  const lines = T.parseFile(e.target.value);
  if (lines.join('\n') !== state.lines.join('\n')) commit(lines);
};

function toggleHelp() {
  const old = $('#help');
  if (old) { old.remove(); return; }
  const rows = [
    ['F1', 'This help'], ['F2', 'Next sort order'], ['F3', 'Show / hide completed'],
    ['F8', 'File / folder selector'], ['F4', 'Show / hide sidebar'], ['F5', 'Archive done tasks'], ['F6', 'Calendar when typing dates on / off'], ['F9', 'Always on top (desktop)'], ['F11', 'Minimal mode (menu bar on hover at the top)'], ['F12', 'Window frame on / off (desktop)'], ['Ctrl+R', 'Reload file from disk'],
    ['Ctrl+S', 'Save'], ['Ctrl+Z', 'Undo (list)'], ['/', 'Search'],
  ];
  const box = el('div', { className: 'help-box' }, el('h2', {}, 'Keys'),
    ...rows.map(([k, t]) => el('div', { className: 'help-row' }, el('kbd', {}, k), ' ', t)));
  const ov = el('div', { id: 'help', className: 'help' }, box);
  ov.onclick = () => ov.remove();
  document.body.append(ov);
}

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && $('#help')) { $('#help').remove(); return; }
  if (e.key === 'Escape' && !settingsEl.hidden) { closeSettings(); return; }
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName);
  if (e.key === 'Escape') closeMenus();
  if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === 't') { e.preventDefault(); reloadTheme(); return; }   // reload theme
  // F-keys: same as the terminal app.
  if (e.key === 'F8' && !e.ctrlKey && !e.altKey && !e.metaKey) {      // file / folder selector
    e.preventDefault();
    const b = $('#fileDd .file-btn');
    if (!b.disabled) b.click();
    return;
  }
  if (e.key === 'F9' && !e.ctrlKey && !e.altKey && !e.metaKey) {   // desktop window effects
    e.preventDefault();
    toggleOnTop();
    return;
  }
  if (e.key === 'F12' && !e.ctrlKey && !e.altKey && !e.metaKey) {      // window frame on/off (desktop)
    e.preventDefault();
    if (isTauri) toggleFrameless();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 'r') {    // reload file from disk
    e.preventDefault();
    checkExternalChange(true);
    return;
  }
  if (e.key === 'F11' && !e.ctrlKey && !e.altKey && !e.metaKey) {      // minimal mode (menus hidden)
    e.preventDefault();
    closeMenus();
    state.minimal = !state.minimal; persist(); render();
    return;
  }
  if (/^F[1-6]$/.test(e.key) && !e.ctrlKey && !e.altKey && !e.metaKey) {
    e.preventDefault();
    const listMode = state.view !== 'text';
    if (e.key === 'F1') toggleHelp();
    else if (e.key === 'F2') {
      const order = ['priority', 'due', 'created', 'alpha', 'file'];
      state.sort = order[(order.indexOf(state.sort) + 1) % order.length];
    } else if (e.key === 'F3') state.showDone = !state.showDone;
    else if (e.key === 'F4') { if (!listMode) return; state.sidebar = !state.sidebar; }
    else if (e.key === 'F5') { archiveDone(); return; }
    else if (e.key === 'F6') state.calendar = !state.calendar;
    if (e.key !== 'F1') { persist(); render(); }
    return;
  }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
    e.preventDefault();
    save({ as: e.shiftKey });
  } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z' && !typing) {
    e.preventDefault();
    undo();
  } else if (!typing && e.key === '/') {
    e.preventDefault();
    composer.setMode('search');
  } else if (!typing && !e.ctrlKey && !e.metaKey && !e.altKey && e.key.length === 1 && e.key !== ' '
             && !document.activeElement?.isContentEditable) {
    // Nothing is taking text input: send typing to the add/search bar.
    // Focusing during keydown lets the same keystroke land in the input.
    $('#addInput').focus();
  }
});

// Drag & drop a file anywhere on the page.
let dragDepth = 0;
const hasFiles = (e) => [...(e.dataTransfer?.types || [])].includes('Files');
window.addEventListener('dragenter', (e) => { if (hasFiles(e)) { dragDepth++; $('#drop').hidden = false; } });
window.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; $('#drop').hidden = true; } });
window.addEventListener('dragover', (e) => { if (hasFiles(e)) e.preventDefault(); });
window.addEventListener('drop', (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth = 0;
  $('#drop').hidden = true;
  const f = e.dataTransfer.files[0];
  if (f) openFromFileObject(f);
});

window.addEventListener('beforeunload', (e) => {
  if (state.dirty) e.preventDefault();
});

restore();
buildThemeMenu();
render();
loadVersion();
// height of the add/search bar for the sidebar (it scrolls above the bar instead of under it)
{
  const form = $('#addForm');
  if (form && 'ResizeObserver' in window) new ResizeObserver(() => document.documentElement.style.setProperty('--composer-h', form.offsetHeight + 'px')).observe(form);
}

// ---------- Android: lists in the app storage / on the SD card, filter drawer, back button ----------
async function androidCreateFile(path) {
  let exists = true;
  try { await tauri.fs.readTextFile(path); } catch { exists = false; }
  if (!exists) await tauri.fs.writeTextFile(path, '');
  state.dirty = false;
  if (state.path === path) return;
  loadText('', baseName(path), null, path);
  rememberFile(path, true);
}
function androidDeps() {
  const swap = (from, to) => { state.files = state.files.map((f) => (f === from ? to : f)).filter((f, i, a) => a.indexOf(f) === i); saveFiles(); };
  return {
    invoke, state, confirmDiscard,
    switchFile: async (p) => { await switchFile(p); },
    createFile: async (p) => { if (!confirmDiscard()) return; await androidCreateFile(p); },
    onMoved: async (from, to, wasCurrent) => { swap(from, to); if (wasCurrent) { state.dirty = false; await switchFile(to); } },
    onGone: async (path) => {
      state.files = state.files.filter((f) => f !== path);
      saveFiles();
      if (path !== state.path) return;
      state.dirty = false;
      for (const d of storageDirs()) {
        let names = [];
        try { names = await invoke('list_txt_files', { dir: d.path }); } catch { /* ignore */ }
        if (names.length) { await switchFile(`${d.path.replace(/\/+$/, '')}/${names[0]}`); return; }
      }
      const home = storageDirs()[0]?.path;
      if (home) { state.path = null; await androidCreateFile(`${home}/todo.txt`); }
    },
  };
}
if (isAndroid) {
  document.body.classList.add('android');
  initBackButton();
  $('#fileDd').addEventListener('click', (e) => {   // the title opens the lists overlay instead of the dropdown
    if (!e.target.closest('.file-btn')) return;
    e.stopPropagation(); closeMenus(); openFiles(androidDeps());
  }, true);
  $('#filterBtn').onclick = () => { state.sidebar = !state.sidebar; render(); };
  $('#scrim').onclick = () => { state.sidebar = false; render(); };
}

openLastFile();
loadAutoSave();
