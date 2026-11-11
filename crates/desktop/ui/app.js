// Vanilla frontend. Relies on `withGlobalTauri: true` - no bundler.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const MAX_LINES = 2000;

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
async function refreshStatus() {
  const s = await invoke("get_status");
  const dot = $("status-dot");
  dot.className = "dot " + (s.following ? "on" : s.token_present ? "off" : "err");

  let text;
  if (!s.token_present) text = "No token set - open Settings";
  else if (s.following) text = "Following " + (s.log_path || "log");
  else text = "Stopped";
  $("status-text").textContent = text;

  const uploads = $("status-uploads");
  const parts = [];
  if (s.upload_count) {
    parts.push(`${s.upload_count} uploads · last ${s.last_endpoint || "?"} @ ${s.last_time || ""}`);
  }
  if (s.upload_failed) {
    parts.push(`${s.upload_failed} failed · last ${s.last_failure || "?"}`);
  }
  uploads.textContent = parts.join(" · ");
  uploads.className = s.upload_failed ? "warn" : "muted";

  $("host").textContent = s.host;
  if (s.app_version) {
    const label = "Recall v" + s.app_version;
    $("brand-name").textContent = label;
    document.title = label;
  }
  if (s.log_path && !$("log-path").value) $("log-path").placeholder = s.log_path;
}

// ---- Settings actions -----------------------------------------------------
$("save-token").addEventListener("click", async () => {
  const msg = $("token-msg");
  try {
    await invoke("save_token", { token: $("token").value });
    msg.className = "msg ok";
    msg.textContent = "Saved. Following started.";
    $("token").value = "";
    setTimeout(() => showView("log"), 600);
  } catch (e) {
    msg.className = "msg err";
    msg.textContent = String(e);
  }
  refreshStatus();
});

$("save-path").addEventListener("click", async () => {
  const msg = $("path-msg");
  try {
    await invoke("set_log_path", { path: $("log-path").value });
    msg.className = "msg ok";
    msg.textContent = "Log path set. Following restarted.";
  } catch (e) {
    msg.className = "msg err";
    msg.textContent = String(e);
  }
  refreshStatus();
});

// ---- Wire up events -------------------------------------------------------
listen("log-line", (e) => appendLine(e.payload));
listen("status-changed", refreshStatus);
listen("show-settings", () => showView("settings"));

(async function init() {
  const lines = await invoke("recent_logs");
  lines.forEach(appendLine);
  // No polling: the backend emits `status-changed` whenever the status moves.
  await refreshStatus();
})();
