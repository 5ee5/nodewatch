"use strict";

// NodeWatch dashboard. Polls /api/stats and updates the DOM in place.
// All server data is written with textContent / setAttribute, never as parsed markup.

(() => {
  const POLL_MS = 2500;
  const ERROR_POLL_MS = 5000;
  const TIMEOUT_MS = 5000;
  const MIN_STALE_MS = 10000;

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

  function level(p) {
    return p >= 90 ? "crit" : p >= 70 ? "warn" : "ok";
  }

  // Bars are driven via a CSS custom property: CSSOM writes are allowed by the CSP,
  // inline style="" attributes are not.
  function setBar(node, p) {
    const v = Math.max(0, Math.min(100, Number(p) || 0));
    node.style.setProperty("--v", String(v));
    node.setAttribute("data-level", level(v));
  }

  function setText(id, text) {
    const node = $(id);
    if (node.textContent !== text) node.textContent = text;
  }

  // ---------- rendering ----------

  function renderCpu(cpu) {
    const big = $("cpu-pct");
    big.textContent = pct(cpu.usage_percent);
    big.appendChild(el("small", null, "%"));
    setBar($("cpu-bar"), cpu.usage_percent);
    setText("cpu-sub", cpu.core_count + (cpu.core_count === 1 ? " core" : " cores") + (cpu.brand ? " · " + cpu.brand : ""));

    const box = $("cpu-cores");
    while (box.children.length < cpu.cores.length) {
      const core = el("div", "core");
      core.appendChild(el("div", "core-fill"));
      box.appendChild(core);
    }
    while (box.children.length > cpu.cores.length) box.removeChild(box.lastChild);
    cpu.cores.forEach((c, i) => {
      const core = box.children[i];
      setBar(core, c.usage_percent);
      const freq = c.frequency_mhz ? " · " + c.frequency_mhz + " MHz" : "";
      core.setAttribute("title", c.name + ": " + pct(c.usage_percent) + "%" + freq);
    });
  }

  function renderMemory(mem) {
    const big = $("mem-pct");
    big.textContent = pct(mem.usage_percent);
    big.appendChild(el("small", null, "%"));
    setBar($("mem-bar"), mem.usage_percent);
    setText("mem-sub", bytes(mem.used_bytes) + " / " + bytes(mem.total_bytes));
    setText("mem-used", bytes(mem.used_bytes));
    setText("mem-avail", bytes(mem.available_bytes));
    setText("mem-total", bytes(mem.total_bytes));
    setText(
      "mem-swap",
      mem.swap_total_bytes > 0 ? bytes(mem.swap_used_bytes) + " / " + bytes(mem.swap_total_bytes) : "none"
    );
  }

  function renderSystem(sys) {
    setText("sys-host", orDash(sys.hostname));
    setText("sys-os", orDash(sys.os));
    setText("sys-kernel", orDash(sys.kernel));
    setText("sys-arch", orDash(sys.cpu_arch));
    setText("sys-uptime", uptime(sys.uptime_secs));
    setText("sys-sub", orDash(sys.hostname));
    setText("host", [sys.hostname, sys.os, sys.kernel].filter(Boolean).join(" · "));
    if (sys.hostname) document.title = sys.hostname + " – NodeWatch";
  }

  let diskKeys = "";

  function renderDisks(disks) {
    const box = $("disks");
    const keys = disks.map((d) => d.mount_point).join("\n");
    if (keys !== diskKeys) {
      // The set of mounts changed (rare): rebuild rows, then fill them below.
      diskKeys = keys;
      box.textContent = "";
      if (disks.length === 0) {
        box.appendChild(el("div", "empty", "No filesystems reported."));
      }
      disks.forEach((d) => {
        const row = el("div", "disk");
        const head = el("div", "disk-head");
        head.appendChild(el("span", "mount", d.mount_point));
        head.appendChild(el("span", "fs", d.file_system));
        if (d.read_only) head.appendChild(el("span", "badge", "ro"));
        head.appendChild(el("span", "disk-pct"));
        row.appendChild(head);
        const bar = el("div", "bar");
        bar.appendChild(el("div", "bar-fill"));
        row.appendChild(bar);
        row.appendChild(el("div", "disk-sub"));
        box.appendChild(row);
      });
    }
    disks.forEach((d, i) => {
      const row = box.children[i];
      if (!row) return;
      row.querySelector(".mount").setAttribute("title", d.name);
      row.querySelector(".disk-pct").textContent = pct(d.usage_percent) + "%";
      setBar(row.querySelector(".bar"), d.usage_percent);
      row.querySelector(".disk-sub").textContent =
        bytes(d.used_bytes) + " used of " + bytes(d.total_bytes) + " · " + bytes(d.available_bytes) + " free";
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
    const pill = $("status");
    pill.setAttribute("data-state", state);
    pill.textContent = text;
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
        if (!hasData) setStatus("loading", "Warming up…");
        banner(hasData ? "Server has no fresh metrics yet." : "", "info");
        delay = POLL_MS;
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
          setStatus("stale", "Stale data");
          banner("The server is reachable but metrics have not updated for " + Math.round((now - lastChangeAt) / 1000) + " s.", "error");
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
        (aborted ? "The server did not respond in time." : "Cannot reach the NodeWatch server.") +
          (hasData ? " Showing the last known values; retrying…" : " Retrying…"),
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

  tick();
})();
