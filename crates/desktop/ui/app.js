// Vanilla frontend. Relies on `withGlobalTauri: true` - no bundler.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const MAX_LINES = 2000;
const SERVICE_NAMES = { seventeenlands: "17Lands", archive: "Local archive" };

// ---- View switching -------------------------------------------------------
function showView(name) {
  document.querySelectorAll(".tab").forEach((t) =>
    t.classList.toggle("active", t.dataset.view === name)
  );
  $("view-log").classList.toggle("hidden", name !== "log");
  $("view-settings").classList.toggle("hidden", name !== "settings");
}
document.querySelectorAll(".tab").forEach((t) =>
  t.addEventListener("click", () => showView(t.dataset.view))
);

// ---- Log feed -------------------------------------------------------------
let showChatter = false;

function appendLine(line) {
  if (line.chatter && !showChatter) return;
  const log = $("log");
  const nearBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;

  const el = document.createElement("span");
  el.className = "l" + (line.chatter ? " chatter" : "");
  const lvl = ["WARN", "ERROR", "DEBUG"].includes(line.level) ? line.level : "";
  el.innerHTML =
    `<span class="ts">${line.ts}</span>` +
    `<span class="${lvl}">${escapeHtml(line.msg)}</span>`;
  log.appendChild(el);

  while (log.childElementCount > MAX_LINES) log.removeChild(log.firstChild);
  if (nearBottom) log.scrollTop = log.scrollHeight;
}

function escapeHtml(s) {
  return s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}

$("show-chatter").addEventListener("change", async (e) => {
  showChatter = e.target.checked;
  $("log").innerHTML = "";
  const lines = await invoke("recent_logs");
  lines.forEach(appendLine);
});

$("clear-log").addEventListener("click", () => ($("log").innerHTML = ""));

// ---- Status ---------------------------------------------------------------
// The last status from the backend, and which inline editors the user has open. A status
// refresh can land at any time (every upload, every window focus), so it never touches what
// the user is typing.
let current = null;
const editing = { token: false, path: false };

const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
const fileName = (path) => path.split(/[\\/]/).pop();

async function refreshStatus() {
  current = await invoke("get_status");
  render();
}

function render() {
  const s = current;
  if (!s) return;
  const state = !s.token_present ? "err" : s.following ? "on" : "off";

  // Header
  $("status-pill").dataset.state = state;
  $("status-text").textContent = { on: "Following", off: "Stopped", err: "Token needed" }[state];
  const stat = $("status-uploads");
  stat.textContent = s.upload_failed
    ? `${s.upload_count} uploaded · ${s.upload_failed} failed`
    : s.upload_count
      ? plural(s.upload_count, "upload")
      : "";
  stat.classList.toggle("warn", s.upload_failed > 0);

  // Log footer
  const last = $("last-upload");
  if (s.upload_failed) {
    last.textContent = `Last failure: ${s.last_failure || "unknown"}`;
  } else if (s.upload_count) {
    last.textContent = `Last upload: ${fileName(s.last_endpoint || "?")} at ${s.last_time || "?"}`;
  } else {
    last.textContent = "";
  }
  last.classList.toggle("warn", s.upload_failed > 0);

  // Settings hero
  $("hero").dataset.state = state;
  let title, sub;
  if (state === "err") {
    title = "Add your 17Lands token";
    sub = "Recall needs it to upload your drafts and games.";
  } else if (state === "on") {
    title = `Following ${fileName(s.log_path || "Player.log")}`;
    sub = "Drafts and games upload as you play.";
  } else if (!s.log_path) {
    title = "Player.log not found";
    sub = "Start Arena once, or point Recall at the log file below.";
  } else {
    title = "Not following";
    sub = "Recall stopped reading the log. The Log tab has the details.";
  }
  $("hero-title").textContent = title;
  $("hero-sub").textContent = sub;
  $("stat-uploads").textContent = s.upload_count;
  $("stat-failed").textContent = s.upload_failed;
  $("stat-failed-box").classList.toggle("hidden", !s.upload_failed);

  // Token row: the editor is forced open until a token exists.
  const tokenOpen = editing.token || !s.token_present;
  $("token-desc").textContent = s.token_present
    ? `Saved · ends in ${s.token_hint}`
    : "Not set";
  $("token-edit").classList.toggle("hidden", tokenOpen);
  $("token-editor").classList.toggle("hidden", !tokenOpen);
  $("token-cancel").classList.toggle("hidden", !s.token_present);

  // Log path row
  $("path-desc").textContent = s.log_path || "Not found";
  $("path-edit").classList.toggle("hidden", editing.path);
  $("path-editor").classList.toggle("hidden", !editing.path);

  // About
  $("host").textContent = s.host;
  $("version").textContent = s.app_version;
  $("services").replaceChildren(
    ...s.services.map((name) => {
      const chip = document.createElement("span");
      chip.className = "chip";
      chip.textContent = SERVICE_NAMES[name] || name;
      return chip;
    })
  );
  document.title = "Recall v" + s.app_version;
}

// ---- Settings actions -----------------------------------------------------
function setMsg(id, text) {
  $(id).textContent = text;
}

function openEditor(kind, inputId, value = "") {
  editing[kind] = true;
  $(inputId).value = value;
  render();
  $(inputId).focus();
  $(inputId).select();
}

function closeEditor(kind, inputId, msgId) {
  editing[kind] = false;
  $(inputId).value = "";
  setMsg(msgId, "");
  render();
}

async function saveToken() {
  try {
    await invoke("save_token", { token: $("token").value });
    closeEditor("token", "token", "token-msg");
  } catch (e) {
    setMsg("token-msg", String(e));
  }
  refreshStatus();
}

async function savePath() {
  try {
    await invoke("set_log_path", { path: $("log-path").value.trim() });
    closeEditor("path", "log-path", "path-msg");
  } catch (e) {
    setMsg("path-msg", String(e));
  }
  refreshStatus();
}

$("token-edit").addEventListener("click", () => openEditor("token", "token"));
$("token-cancel").addEventListener("click", () => closeEditor("token", "token", "token-msg"));
$("save-token").addEventListener("click", saveToken);
$("path-edit").addEventListener("click", () =>
  openEditor("path", "log-path", current?.log_path || "")
);
$("path-cancel").addEventListener("click", () => closeEditor("path", "log-path", "path-msg"));
$("save-path").addEventListener("click", savePath);

// Enter saves, Escape cancels (where there is something to go back to).
function editorKeys(inputId, save, cancelId) {
  $(inputId).addEventListener("keydown", (e) => {
    if (e.key === "Enter") save();
    else if (e.key === "Escape" && !$(cancelId).classList.contains("hidden")) $(cancelId).click();
  });
}
editorKeys("token", saveToken, "token-cancel");
editorKeys("log-path", savePath, "path-cancel");

// ---- Wire up events -------------------------------------------------------
listen("log-line", (e) => appendLine(e.payload));
listen("status-changed", refreshStatus);
// First run without a token: the backend opens the window straight on Settings.
listen("show-settings", () => {
  showView("settings");
  $("token").focus();
});

(async function init() {
  const lines = await invoke("recent_logs");
  lines.forEach(appendLine);
  // No polling: the backend emits `status-changed` whenever the status moves.
  await refreshStatus();
})();
