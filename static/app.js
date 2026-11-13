"use strict";

// NodeWatch dashboard. Polls /api/stats and updates the DOM in place.
// All server data is written with textContent / setAttribute, never as parsed markup.

(() => {
  const POLL_MS = 2500;
  const ERROR_POLL_MS = 5000;
  const TIMEOUT_MS = 5000;
  const MIN_STALE_MS = 10000;
  const GHOST_CELLS = 16; // placeholder cells shown until the first sample arrives

  const $ = (id) => document.getElementById(id);
  const grid = $("grid");

  let timer = null;
  let hasData = false;
  let lastSampleTs = null; // server's collected_at_ms of the newest sample seen
  let lastChangeAt = 0; // local monotonic time when that value last changed

  // ---------- formatting ----------

  const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

  function bytes(n) {
    if (typeof n !== "number" || !isFinite(n)) return "–";
    let i = 0;
    while (n >= 1024 && i < UNITS.length - 1) { n /= 1024; i++; }
    return (i === 0 ? n.toFixed(0) : n.toFixed(1)) + " " + UNITS[i];
  }

  function pct(n) {
    return typeof n === "number" && isFinite(n) ? n.toFixed(1) : "–";
  }

  function uptime(secs) {
    if (typeof secs !== "number") return "–";
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (d > 0) return d + "d " + h + "h " + m + "m";
    if (h > 0) return h + "h " + m + "m";
    return m + "m " + Math.floor(secs % 60) + "s";
  }

  const orDash = (v) => (v === null || v === undefined || v === "" ? "–" : String(v));

  // ---------- DOM helpers ----------

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function setText(id, text) {
    const node = $(id);
    if (node.textContent !== text) node.textContent = text;
  }

  // Meters and core cells: healthy below 70%, busy to 90%, saturated beyond.
  function level(p) {
    return p >= 90 ? "crit" : p >= 70 ? "warn" : "ok";
  }

  // Bars are driven via a CSS custom property: CSSOM writes are allowed by the CSP,
  // inline style="" attributes are not. `node` is a .meter or .core; CSS draws the fill from --v.
  function setMeter(node, p) {
    const v = Math.max(0, Math.min(100, Number(p) || 0));
    node.style.setProperty("--v", String(v));
    node.setAttribute("data-level", level(v));
  }

  // ---------- core strip ----------

  let cores = []; // latest per-core data
  let selectedCore = null; // pinned by click / tap / keyboard
  let hoveredCore = null; // transient, desktop pointer only

  const wall = $("cpu-cores");

  function coreLabel(c, i) {
    const m = /^cpu(\d+)$/i.exec(c.name || "");
    return m ? m[1] : String(i);
  }

  function describeCore(c) {
    const freq = c.frequency_mhz ? ", " + c.frequency_mhz + " MHz" : "";
    return c.name + ": " + pct(c.usage_percent) + "% busy" + freq;
  }

  function renderCaption() {
    const idx = hoveredCore !== null ? hoveredCore : selectedCore;
    let text = "";
    if (idx !== null && cores[idx]) {
      text = describeCore(cores[idx]);
    } else if (cores.length) {
      let busiest = 0;
      cores.forEach((c, i) => { if (c.usage_percent > cores[busiest].usage_percent) busiest = i; });
      text = cores[busiest].usage_percent < 1
        ? "All CPUs are idle. Select a core for its clock speed."
        : "Busiest: " + cores[busiest].name + " at " + pct(cores[busiest].usage_percent) + "%. Select a core for its clock speed.";
    }
    setText("core-detail", text);
  }

  function ensureCells(n, ghost) {
    while (wall.children.length < n) {
      const cell = el("button", "core");
      cell.type = "button";
      cell.appendChild(el("span", "core-idx"));
      cell.appendChild(el("span", "core-val"));
      wall.appendChild(cell);
    }
    while (wall.children.length > n) wall.removeChild(wall.lastChild);
    if (ghost) {
      for (const cell of wall.children) cell.disabled = true;
    }
  }

  function renderCores(list) {
    cores = list;
    ensureCells(list.length, false);
    if (selectedCore !== null && selectedCore >= list.length) selectedCore = null;
    list.forEach((c, i) => {
      const cell = wall.children[i];
      cell.disabled = false;
      cell.children[0].textContent = coreLabel(c, i);
      cell.children[1].textContent = Math.round(c.usage_percent) + "%";
      setMeter(cell, c.usage_percent);
      cell.setAttribute("aria-pressed", selectedCore === i ? "true" : "false");
      cell.setAttribute("aria-label", c.name + ", " + pct(c.usage_percent) + " percent busy");
    });
    renderCaption();
  }

  function cellIndex(target) {
    const cell = target.closest && target.closest(".core");
    return cell && wall.contains(cell) ? Array.prototype.indexOf.call(wall.children, cell) : -1;
  }

  wall.addEventListener("click", (e) => {
    const i = cellIndex(e.target);
    if (i < 0 || !cores[i]) return;
    selectedCore = selectedCore === i ? null : i;
    cores.forEach((_, n) => wall.children[n].setAttribute("aria-pressed", selectedCore === n ? "true" : "false"));
    renderCaption();
  });

  wall.addEventListener("mouseover", (e) => {
    const i = cellIndex(e.target);
    hoveredCore = i >= 0 && cores[i] ? i : null;
    renderCaption();
  });

  wall.addEventListener("mouseleave", () => {
    hoveredCore = null;
    renderCaption();
  });

  // ---------- rendering ----------

  function renderCpu(cpu) {
    setText("cpu-pct", pct(cpu.usage_percent));
    setMeter($("cpu-bar"), cpu.usage_percent);
    const threads = cpu.core_count + " logical " + (cpu.core_count === 1 ? "CPU" : "CPUs");
    setText("cpu-sub", cpu.brand ? cpu.brand + ", " + threads : threads);
    renderCores(cpu.cores);
  }

  function renderMemory(mem) {
    setText("mem-pct", pct(mem.usage_percent));
    setMeter($("mem-bar"), mem.usage_percent);
    setText("mem-sub", bytes(mem.used_bytes) + " of " + bytes(mem.total_bytes));
    setText("mem-used", bytes(mem.used_bytes));
    setText("mem-avail", bytes(mem.available_bytes));
    setText("mem-total", bytes(mem.total_bytes));

    const hasSwap = mem.swap_total_bytes > 0;
    $("swap-bar").hidden = !hasSwap;
    setText("swap-text", hasSwap ? bytes(mem.swap_used_bytes) + " of " + bytes(mem.swap_total_bytes) : "None configured");
    if (hasSwap) setMeter($("swap-bar"), (mem.swap_used_bytes / mem.swap_total_bytes) * 100);
  }

  function renderSystem(sys) {
    setText("sys-os", orDash(sys.os));
    setText("sys-kernel", orDash(sys.kernel));
    setText("sys-arch", orDash(sys.cpu_arch));
    setText("sys-uptime", uptime(sys.uptime_secs));
    document.title = (sys.hostname ? sys.hostname + " – " : "") + "NodeWatch";
  }

  let diskKeys = "";

  function buildDiskRow(d) {
    const row = el("div", "disk");

    const id = el("div", "disk-id");
    id.appendChild(el("span", "mount", d.mount_point));
    id.appendChild(el("span", "fs", d.file_system + (d.read_only ? ", read-only" : "")));
    row.appendChild(id);

    const meter = el("div", "meter");
    meter.appendChild(el("i"));
    row.appendChild(meter);

    const amount = el("div", "disk-amount");
    amount.appendChild(el("span", "disk-used"));
    amount.appendChild(el("span", "disk-free"));
    row.appendChild(amount);

    row.appendChild(el("div", "disk-pct"));
    return row;
  }

  function renderDisks(disks) {
    const box = $("disks");
    const keys = disks.map((d) => d.mount_point).join("\n");
    if (keys !== diskKeys) {
      // The set of mounts changed (rare): rebuild the rows, then fill them below.
      diskKeys = keys;
      box.textContent = "";
      if (disks.length === 0) box.appendChild(el("p", "empty", "No filesystems reported."));
      disks.forEach((d) => box.appendChild(buildDiskRow(d)));
    }
    disks.forEach((d, i) => {
      const row = box.children[i];
      if (!row) return;
      row.querySelector(".mount").setAttribute("title", d.name);
      row.querySelector(".disk-pct").textContent = pct(d.usage_percent) + "%";
      setMeter(row.querySelector(".meter"), d.usage_percent);
      row.querySelector(".disk-used").textContent = bytes(d.used_bytes) + " of " + bytes(d.total_bytes);
      row.querySelector(".disk-free").textContent = bytes(d.available_bytes) + " free";
    });
    setText("disks-sub", disks.length + (disks.length === 1 ? " filesystem" : " filesystems"));
  }

  function render(data) {
    renderSystem(data.system);
    renderCpu(data.cpu);
    renderMemory(data.memory);
    renderDisks(data.disks);
  }

  // ---------- status ----------

  function setStatus(state, text) {
    $("status").setAttribute("data-state", state);
    setText("status-text", text);
    grid.classList.toggle("is-stale", state === "stale" || state === "offline");
  }

  function banner(message, kind) {
    const node = $("banner");
    if (!message) {
      node.hidden = true;
      return;
    }
    node.textContent = message;
    node.setAttribute("data-kind", kind || "error");
    node.hidden = false;
  }

  function markLoaded() {
    if (hasData) return;
    hasData = true;
    grid.classList.remove("is-loading");
    grid.setAttribute("aria-busy", "false");
  }

  // ---------- polling ----------

  async function fetchStats() {
    const ctl = new AbortController();
    const t = setTimeout(() => ctl.abort(), TIMEOUT_MS);
    try {
      return await fetch("/api/stats", {
        headers: { Accept: "application/json" },
        cache: "no-store",
        signal: ctl.signal,
      });
    } finally {
      clearTimeout(t);
    }
  }

  async function tick() {
    let delay = POLL_MS;
    try {
      const res = await fetchStats();
      if (res.status === 401) {
        location.assign("/login");
        return;
      }
      if (res.status === 503) {
        // Server is up but has not produced a sample yet.
        if (!hasData) setStatus("loading", "Starting up");
        banner(hasData ? "The server has no fresh metrics yet." : "", "info");
      } else if (!res.ok) {
        throw new Error("HTTP " + res.status);
      } else {
        const data = await res.json();
        const now = performance.now();
        if (data.collected_at_ms !== lastSampleTs) {
          lastSampleTs = data.collected_at_ms;
          lastChangeAt = now;
        }
        render(data);
        markLoaded();

        // Stale = the server keeps answering but the sampler stopped producing samples.
        const staleAfter = Math.max(MIN_STALE_MS, 3 * 1000 * (data.interval_secs || 0));
        if (now - lastChangeAt > staleAfter) {
          setStatus("stale", "Stale");
          banner(
            "Metrics haven't updated for " + Math.round((now - lastChangeAt) / 1000) +
              " seconds. The server is reachable, but its sampler may be stuck.",
            "error"
          );
        } else {
          setStatus("live", "Live");
          banner("");
        }
        setText("updated", "Updated " + new Date().toLocaleTimeString());
      }
    } catch (err) {
      const aborted = err && err.name === "AbortError";
      setStatus("offline", "Offline");
      banner(
        (aborted ? "The server took too long to respond." : "Can't reach the NodeWatch server.") +
          (hasData ? " Showing the last values received. Retrying every 5 seconds." : " Retrying every 5 seconds."),
        "error"
      );
      delay = ERROR_POLL_MS;
    }
    schedule(delay);
  }

  function schedule(ms) {
    clearTimeout(timer);
    timer = setTimeout(tick, ms);
  }

  // Do not poll from background tabs; refresh immediately on return.
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      clearTimeout(timer);
    } else {
      schedule(0);
    }
  });

  $("logout").addEventListener("click", async () => {
    $("logout").disabled = true;
    try {
      await fetch("/api/logout", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
    } catch (_) {
      // Even if the request failed, leave; the session expires server-side.
    }
    location.assign("/login");
  });

  ensureCells(GHOST_CELLS, true);
  tick();
})();
