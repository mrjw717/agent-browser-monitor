import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import appIcon from "./app-icon.png";
import "./style.css";

type ProcessInfo = {
  pid: number;
  ppid: number;
  name: string;
  state: string;
  cpu_percent: number;
  memory_mib: number;
  active_seconds: number;
  elapsed_seconds: number;
  command: string;
};

type Session = {
  id: string;
  kind: string;
  profile: string | null;
  root_pid: number;
  agent_pid: number | null;
  processes: ProcessInfo[];
  cpu_percent: number;
  memory_mib: number;
  active_seconds: number;
};

type Snapshot = {
  sessions: Session[];
  agent_process_count: number;
  chrome_process_count: number;
  cpu_busy_percent: number;
  memory_available_mib: number;
  swap_used_mib: number;
  platform: string;
  captured_at: string;
};

const app = document.querySelector<HTMLDivElement>("#app")!;
let snapshot: Snapshot | null = null;
let expanded = new Set<string>();
let refreshing = false;

const formatAge = (seconds: number) => {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
};

const num = (value: number, digits = 0) => value.toLocaleString(undefined, { maximumFractionDigits: digits });
const memoryGb = (mib: number) => `${num(mib / 1024, mib >= 1024 ? 1 : 2)} GB`;

function render() {
  if (!snapshot) {
    app.innerHTML = `<main><p class="loading">Reading local processes…</p></main>`;
    return;
  }

  const sessionRows = snapshot.sessions.length
    ? snapshot.sessions.map((session) => {
      const open = expanded.has(session.id);
      const kind = session.kind;
      const profile = session.profile ? session.profile.replace("/tmp/agent-browser-chrome-", "profile ") : "unattributed agent process";
      const details = open ? `
        <div class="details">
          <div class="detail-heading"><span>${session.processes.length} process${session.processes.length === 1 ? "" : "es"}</span><span>Command lines are locally redacted for URLs and tokens.</span></div>
          ${session.processes.map((process) => `
            <div class="process">
              <div><strong>${process.name}</strong><span>PID ${process.pid} · parent ${process.ppid} · ${process.state} · ${formatAge(process.elapsed_seconds)}</span></div>
              <div class="process-metrics">${num(process.cpu_percent, 1)}% CPU · ${num(process.memory_mib, 1)} MiB</div>
              <code>${escapeHtml(process.command)}</code>
            </div>`).join("")}
        </div>` : "";
      return `
        <section class="session">
          <div class="session-main">
            <button class="expander" data-expand="${session.id}" aria-label="${open ? "Hide" : "Show"} process details">${open ? "−" : "+"}</button>
            <div class="session-title"><strong>${kind}</strong><span>${escapeHtml(profile)}</span></div>
            <div class="metric"><b>${num(session.cpu_percent, 1)}%</b><span>CPU</span></div>
            <div class="metric"><b>${memoryGb(session.memory_mib)}</b><span>RAM</span></div>
            <div class="metric"><b>${session.processes.length}</b><span>processes</span></div>
            <div class="metric"><b>${formatAge(session.active_seconds)}</b><span>active</span></div>
            <button class="terminate" data-terminate="${session.id}">Terminate…</button>
          </div>
          ${details}
        </section>`;
    }).join("")
    : `<div class="empty"><strong>No agent-browser processes found.</strong><span>Nothing is using a /tmp/agent-browser-chrome-* profile right now.</span></div>`;

  app.innerHTML = `
    <main>
      <div class="titlebar" data-tauri-drag-region>
        <div class="app-mark" data-tauri-drag-region><img src="${appIcon}" alt="" />Agent Browser Monitor</div>
        <div class="window-controls">
          <button data-window="minimize" data-tauri-drag-region="false" aria-label="Minimize">−</button>
          <button data-window="maximize" data-tauri-drag-region="false" aria-label="Maximize">□</button>
          <button class="window-close" data-window="close" data-tauri-drag-region="false" aria-label="Close">×</button>
        </div>
      </div>
      <header>
        <div><p class="eyebrow">LOCAL PROCESS CONTROL</p><h1>Agent Browser Monitor</h1><p class="subtitle">Only detects agent-browser and its temporary headless Chrome profiles.</p></div>
        <button id="refresh" class="refresh" ${refreshing ? "disabled" : ""}>${refreshing ? "Refreshing…" : "Refresh"}</button>
      </header>
      <section class="summary">
        <div><span>Host CPU</span><strong>${snapshot.platform === "linux" ? `${num(snapshot.cpu_busy_percent, 1)}%` : "—"}</strong></div>
        <div><span>Available RAM</span><strong>${snapshot.platform === "linux" ? memoryGb(snapshot.memory_available_mib) : "—"}</strong></div>
        <div><span>Swap used</span><strong>${snapshot.platform === "linux" ? `${num(snapshot.swap_used_mib)} MiB` : "—"}</strong></div>
        <div><span>Agent / Chrome</span><strong>${snapshot.agent_process_count} / ${snapshot.chrome_process_count}</strong></div>
      </section>
      <div class="list-heading"><span>SESSIONS</span><span>Updated ${new Date(snapshot.captured_at).toLocaleTimeString()}</span></div>
      <div class="sessions">${sessionRows}</div>
      <footer>Termination is limited to the selected, verified agent-browser or Playwright process tree. Unix uses TERM then KILL; Windows uses taskkill for that verified tree.</footer>
    </main>`;

  document.querySelector("#refresh")?.addEventListener("click", () => void refresh());
  document.querySelectorAll<HTMLButtonElement>("[data-window]").forEach((button) => button.addEventListener("click", () => {
    const window = getCurrentWindow();
    const action = button.dataset.window;
    if (action === "minimize") void window.minimize();
    if (action === "maximize") void window.toggleMaximize();
    if (action === "close") void window.close();
  }));
  document.querySelectorAll<HTMLButtonElement>("[data-expand]").forEach((button) => button.addEventListener("click", () => {
    const id = button.dataset.expand!;
    expanded.has(id) ? expanded.delete(id) : expanded.add(id);
    render();
  }));
  document.querySelectorAll<HTMLButtonElement>("[data-terminate]").forEach((button) => button.addEventListener("click", () => void terminate(button.dataset.terminate!)));
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
}

async function refresh() {
  refreshing = true;
  render();
  try {
    snapshot = await invoke<Snapshot>("get_snapshot");
  } catch (error) {
    alert(`Could not read local processes: ${String(error)}`);
  } finally {
    refreshing = false;
    render();
  }
}

async function terminate(id: string) {
  const session = snapshot?.sessions.find((item) => item.id === id);
  if (!session || !confirm(`Terminate this verified ${session.agent_pid ? "agent-browser session" : "orphaned Chrome profile"}?\n\n${session.processes.length} process(es) will be stopped.`)) return;
  try {
    await invoke("terminate_session", { sessionId: id });
    expanded.delete(id);
    await refresh();
  } catch (error) {
    alert(`Could not terminate this session: ${String(error)}`);
  }
}

void refresh();
window.setInterval(() => void refresh(), 3000);
